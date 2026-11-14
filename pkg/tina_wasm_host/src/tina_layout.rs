//! The layout-strategy plugin leg (`tina:layout@1.0.0`; tina's
//! `docs/layout-plugin.md`). The TUI's `tina:layout/bridge` host import
//! routes `solve` calls here, and here alone: the mounted strategy
//! component runs in its own Store, under its own epoch deadline, with
//! zero capabilities — admission refuses any import beyond WASI and the
//! dart2wasm trace/timeline stubs, so a layout plugin cannot reach the
//! session store, the filesystem, the network or the engine services.
//!
//! A trap (including the epoch deadline killing a wedged solver) retires
//! the Store; the next call re-instantiates from the artifact bytes. A
//! wedged layout plugin costs one deadline interval, never the screen:
//! the bridge reports the failure and the guest falls back to its
//! compiled-in solver.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use wasmtime::component::{Component, Func, Linker, Val};
use wasmtime::{Engine, Result, Store, StoreLimits, StoreLimitsBuilder, Trap};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::plugin_loader::{
    plugin_engine_config, verify_embedder_abi, PluginLimits, DEFAULT_EPOCH_TICK,
    DEFAULT_INSTANTIATION_DEADLINE,
};

/// The interface a layout plugin must export (and the only one accepted).
pub const LAYOUT_STRATEGY_INTERFACE: &str = "tina:layout/strategy@1.0.0";
/// The bridge the TUI imports; served from [`LayoutBridge`].
pub const LAYOUT_BRIDGE_INTERFACE: &str = "tina:layout/bridge@1.0.0";

/// Wall-clock default for one layout solve. Layout rides resize paths, so
/// this is far tighter than the engine-plugin default: a strategy that
/// cannot answer in a frame's budget is a strategy the screen cannot use.
pub const DEFAULT_LAYOUT_CALL_DEADLINE: Duration = Duration::from_millis(250);

// ---------------------------------------------------------------------------
// Domain types (the boundary records; see tina's LayoutWire for the JSON
// forms and the WIT in both repos for the contract).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutTrackKind {
    Fixed,
    Weighted,
    Auto,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutTrack {
    pub kind: LayoutTrackKind,
    pub size_units: f64,
    pub weight: f64,
    pub min_units: f64,
    /// `None` = unbounded.
    pub max_units: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutSpan {
    pub start: Option<u32>,
    pub span: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LayoutItem {
    pub id: String,
    pub column: LayoutSpan,
    pub row: LayoutSpan,
    pub min_width_units: f64,
    pub min_height_units: f64,
    pub max_width_units: Option<f64>,
    pub max_height_units: Option<f64>,
    pub aspect_hint: Option<f64>,
    pub priority: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LayoutSpec {
    pub columns: Vec<LayoutTrack>,
    pub rows: Vec<LayoutTrack>,
    pub items: Vec<LayoutItem>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    pub rows_per_unit: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StrategyDescriptor {
    pub id: String,
    pub version: String,
    pub description: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellRect {
    pub col: i32,
    pub row: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LayoutDrop {
    pub id: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LayoutSolution {
    pub columns: u32,
    pub rows: u32,
    pub rows_per_unit: f64,
    /// Item-declaration order (the WIT `rects` list; there is no map type).
    pub rects: Vec<(String, CellRect)>,
    pub column_track_sizes: Vec<f64>,
    pub row_track_sizes: Vec<f64>,
    pub dropped: Vec<LayoutDrop>,
    pub overflow_units_x: f64,
    pub overflow_units_y: f64,
    pub trailing_gap_units_x: f64,
    pub trailing_gap_units_y: f64,
}

/// One solve request: the bridge marshals this from the guest record.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutRequest {
    pub spec: LayoutSpec,
    pub columns: u32,
    pub rows: u32,
    pub metrics: CellMetrics,
}

/// Why the bridge could not produce a plugin solution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutFallback {
    NoPlugin,
    PluginError(String),
    PluginTimeout(String),
}

// ---------------------------------------------------------------------------
// The plugin runtime: one artifact, one Store, epoch-deadline calls.
// ---------------------------------------------------------------------------

struct LayoutPluginStore {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
}

impl WasiView for LayoutPluginStore {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

struct LivePlugin {
    store: Store<LayoutPluginStore>,
    describe: Func,
    solve: Func,
}

/// The mounted layout-strategy plugin. One instance per artifact; the
/// bridge is its only caller. All state is behind one mutex and calls are
/// synchronous (the TUI guest's solve import is sync, like tty.get-size).
pub struct LayoutPluginRuntime {
    engine: Engine,
    limits: PluginLimits,
    /// The dev loop: re-read the artifact before each call so a rebuilt
    /// strategy takes effect on the next solve (hot swap).
    artifact: Option<std::path::PathBuf>,
    bytes: Mutex<Vec<u8>>,
    live: Mutex<Option<LivePlugin>>,
    _ticker: EpochTicker,
}

impl LayoutPluginRuntime {
    /// Mount from an artifact path. `watch` re-reads the file per call.
    pub fn from_artifact(
        path: &std::path::Path,
        watch: bool,
        limits: PluginLimits,
    ) -> Result<Self> {
        let bytes = std::fs::read(path)
            .map_err(|e| wasmtime::Error::msg(format!("reading layout artifact: {e}")))?;
        Self::new(
            bytes,
            watch.then(|| path.to_path_buf()),
            limits,
        )
    }

    pub fn new(bytes: Vec<u8>, artifact: Option<std::path::PathBuf>, limits: PluginLimits) -> Result<Self> {
        verify_embedder_abi(&bytes)
            .map_err(|e| e.context("layout plugin embedder ABI preflight"))?;
        let engine = Engine::new(&plugin_engine_config())?;
        let ticker = EpochTicker::spawn(&engine, limits.epoch_tick)?;
        Ok(Self {
            engine,
            limits,
            artifact,
            bytes: Mutex::new(bytes),
            live: Mutex::new(None),
            _ticker: ticker,
        })
    }

    /// The mounted strategy's descriptor; proves the export answers.
    pub fn describe(&self) -> Result<StrategyDescriptor, LayoutFallback> {
        let mut guard = self.live.lock().map_err(poisoned)?;
        self.ensure(&mut guard)?;
        let live = guard.as_mut().expect("ensure loaded a plugin");
        let ticks = self.call_ticks();
        live.store.set_epoch_deadline(ticks);
        let mut results = [Val::Record(Vec::new())];
        let call = live.describe.call(&mut live.store, &[], &mut results);
        if let Err(error) = call {
            *guard = None; // retire
            return Err(classify(error, "describe"));
        }
        parse_descriptor(results.into_iter().next())
    }

    /// Solve one request through the plugin Store.
    pub fn solve(&self, request: &LayoutRequest) -> Result<LayoutSolution, LayoutFallback> {
        let mut guard = self.live.lock().map_err(poisoned)?;
        self.ensure(&mut guard)?;
        let live = guard.as_mut().expect("ensure loaded a plugin");
        let params = [
            spec_val(&request.spec),
            Val::U32(request.columns),
            Val::U32(request.rows),
            Val::Record(vec![("rows-per-unit".to_string(), Val::Float64(request.metrics.rows_per_unit))]),
        ];
        let mut results = [Val::Result(Ok(None))];
        live.store.set_epoch_deadline(self.call_ticks());
        let call = live.solve.call(&mut live.store, &params, &mut results);
        if let Err(error) = call {
            *guard = None; // retire; the next call re-instantiates
            return Err(classify(error, "solve"));
        }
        let value = results.into_iter().next().ok_or_else(|| {
            LayoutFallback::PluginError("solve omitted its result".into())
        })?;
        let Val::Result(result) = value else {
            return Err(LayoutFallback::PluginError("solve returned a non-result".into()));
        };
        match result {
            Ok(Some(payload)) => parse_solution(*payload),
            Ok(None) => Err(LayoutFallback::PluginError(
                "solve result omitted success payload".into(),
            )),
            Err(Some(payload)) => match *payload {
                Val::String(message) => Err(LayoutFallback::PluginError(message)),
                _ => Err(LayoutFallback::PluginError(
                    "solve error payload was not a string".into(),
                )),
            },
            Err(None) => Err(LayoutFallback::PluginError(
                "solve result omitted error payload".into(),
            )),
        }
    }

    /// Reload the artifact bytes now (watch mode reads before calls too).
    pub fn reload(&self) -> Result<(), LayoutFallback> {
        let Some(artifact) = &self.artifact else {
            return Ok(());
        };
        let fresh = std::fs::read(artifact).map_err(|e| {
            LayoutFallback::PluginError(format!("re-reading layout artifact: {e}"))
        })?;
        let mut bytes = self.bytes.lock().map_err(poisoned)?;
        *bytes = fresh;
        let mut live = self.live.lock().map_err(poisoned)?;
        *live = None; // fresh bytes -> fresh Store on the next call
        Ok(())
    }

    /// Instantiate the plugin if no live Store exists (first call, or a
    /// fresh load after a trap/reload).
    fn ensure(&self, guard: &mut Option<LivePlugin>) -> Result<(), LayoutFallback> {
        if guard.is_some() {
            return Ok(());
        }
        // Watch mode: pick up a rebuilt artifact before instantiating.
        if let Some(artifact) = &self.artifact {
            if let Ok(fresh) = std::fs::read(artifact) {
                if let Ok(mut bytes) = self.bytes.lock() {
                    *bytes = fresh;
                }
            }
        }
        let bytes = self.bytes.lock().map_err(poisoned)?.clone();
        let component = Component::new(&self.engine, &bytes)
            .map_err(|e| LayoutFallback::PluginError(format!("compiling layout plugin: {e}")))?;
        admit(&self.engine, &component)?;
        let mut linker: Linker<LayoutPluginStore> = Linker::new(&self.engine);
        wasmtime_wasi::p3::add_to_linker::<LayoutPluginStore>(&mut linker)
            .map_err(|e| LayoutFallback::PluginError(format!("wasi linker: {e}")))?;
        {
            let mut root = linker.root();
            let mut trace = root
                .instance("wasm:dart/trace@1.0.0")
                .map_err(|e| LayoutFallback::PluginError(format!("trace instance: {e}")))?;
            trace
                .func_wrap("capture-utf16", |_store, (_capacity,): (u32,)| {
                    Ok((String::new(),))
                })
                .map_err(|e| LayoutFallback::PluginError(format!("trace stub: {e}")))?;
            let mut timeline = root
                .instance("wasm:dart/timeline@1.0.0")
                .map_err(|e| LayoutFallback::PluginError(format!("timeline instance: {e}")))?;
            timeline
                .func_wrap(
                    "record-task-event",
                    |_store, (_ty, _task, _flow, _name, _args): (u8, i32, i32, String, String)| {
                        Ok((true,))
                    },
                )
                .map_err(|e| LayoutFallback::PluginError(format!("timeline stub: {e}")))?;
        }
        let mut store = Store::new(
            &self.engine,
            LayoutPluginStore {
                wasi: WasiCtxBuilder::new().build(),
                table: ResourceTable::new(),
                limits: StoreLimitsBuilder::new()
                    .memory_size(self.limits.memory_bytes)
                    .table_elements(self.limits.table_elements)
                    .instances(8)
                    .memories(8)
                    .tables(16)
                    .trap_on_grow_failure(true)
                    .build(),
            },
        );
        store.limiter(|data| &mut data.limits);
        // Instantiation is guest execution: it runs under the same budget.
        store.set_epoch_deadline(self.instantiation_ticks());
        let instance = linker
            .instantiate(&mut store, &component)
            .map_err(|e| LayoutFallback::PluginError(format!("instantiating layout plugin: {e}")))?;
        let export = component
            .get_export_index(None, LAYOUT_STRATEGY_INTERFACE)
            .ok_or_else(|| {
                LayoutFallback::PluginError(format!(
                    "missing {LAYOUT_STRATEGY_INTERFACE} export"
                ))
            })?;
        let describe_index = instance
            .get_export_index(&mut store, Some(&export), "describe")
            .ok_or_else(|| LayoutFallback::PluginError("missing describe export".into()))?;
        let solve_index = instance
            .get_export_index(&mut store, Some(&export), "solve")
            .ok_or_else(|| LayoutFallback::PluginError("missing solve export".into()))?;
        let describe = instance
            .get_func(&mut store, &describe_index)
            .ok_or_else(|| LayoutFallback::PluginError("describe is not callable".into()))?;
        let solve = instance
            .get_func(&mut store, &solve_index)
            .ok_or_else(|| LayoutFallback::PluginError("solve is not callable".into()))?;
        *guard = Some(LivePlugin { store, describe, solve });
        Ok(())
    }

    fn call_ticks(&self) -> u64 {
        ticks_for(self.limits.epoch_tick, self.limits.call_deadline)
    }

    fn instantiation_ticks(&self) -> u64 {
        ticks_for(self.limits.epoch_tick, self.limits.instantiation_deadline)
    }
}

fn ticks_for(tick: Duration, deadline: Duration) -> u64 {
    let tick = tick.as_nanos().max(1);
    u64::try_from(deadline.as_nanos().div_ceil(tick)).unwrap_or(u64::MAX).max(1)
}

fn poisoned<T>(_: std::sync::PoisonError<T>) -> LayoutFallback {
    LayoutFallback::PluginError("layout runtime lock poisoned".into())
}

/// Classify a failed guest call: an epoch-deadline kill is a timeout
/// (the strategy ran past its budget); everything else is an error.
fn classify(error: wasmtime::Error, what: &str) -> LayoutFallback {
    if error.chain().any(|cause| {
        cause
            .downcast_ref::<Trap>()
            .is_some_and(|trap| matches!(trap, Trap::Interrupt))
    }) || error.to_string().contains("epoch deadline") {
        LayoutFallback::PluginTimeout(format!("{what} exceeded the deadline"))
    } else {
        LayoutFallback::PluginError(format!("{what} trapped: {error}"))
    }
}

/// Structural admission: the strategy interface must be exported, and the
/// component must import NOTHING beyond WASI and the dart2wasm
/// trace/timeline stubs. This is the zero-capability claim, enforced.
fn admit(engine: &Engine, component: &Component) -> Result<(), LayoutFallback> {
    let ty = component.component_type();
    for (name, _item) in ty.imports(engine) {
        let module = name;
        // `tina:layout/types@1.0.0` is the contract's own vocabulary: a
        // types-only interface (no functions to call through) that WIT
        // requires worlds to import when an exported interface `use`s its
        // records. Everything else - session stores, filesystem, network,
        // engine services, any foreign tina:* surface - is refused.
        let allowed = module.starts_with("wasi:")
            || module == "wasm:dart/trace@1.0.0"
            || module == "wasm:dart/timeline@1.0.0"
            || module == "tina:layout/types@1.0.0";
        if !allowed {
            return Err(LayoutFallback::PluginError(format!(
                "layout plugin imports forbidden interface {module} \
                 (zero-capability admission)"
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Val marshalling (the boundary records, in WIT declaration order).
// ---------------------------------------------------------------------------

fn track_val(track: &LayoutTrack) -> Val {
    Val::Record(vec![
        (
            "kind".into(),
            Val::Enum(match track.kind {
                LayoutTrackKind::Fixed => "fixed".to_string(),
                LayoutTrackKind::Weighted => "weighted".to_string(),
                LayoutTrackKind::Auto => "auto".to_string(),
            }),
        ),
        ("size-units".into(), Val::Float64(track.size_units)),
        ("weight".into(), Val::Float64(track.weight)),
        ("min-units".into(), Val::Float64(track.min_units)),
        (
            "max-units".into(),
            Val::Option(track.max_units.map(|v| Box::new(Val::Float64(v)))),
        ),
    ])
}

fn span_val(span: &LayoutSpan) -> Val {
    Val::Record(vec![
        (
            "start".into(),
            Val::Option(span.start.map(|v| Box::new(Val::U32(v)))),
        ),
        ("span".into(), Val::U32(span.span)),
    ])
}

fn item_val(item: &LayoutItem) -> Val {
    Val::Record(vec![
        ("id".into(), Val::String(item.id.clone())),
        ("column".into(), span_val(&item.column)),
        ("row".into(), span_val(&item.row)),
        ("min-width-units".into(), Val::Float64(item.min_width_units)),
        ("min-height-units".into(), Val::Float64(item.min_height_units)),
        (
            "max-width-units".into(),
            Val::Option(item.max_width_units.map(|v| Box::new(Val::Float64(v)))),
        ),
        (
            "max-height-units".into(),
            Val::Option(item.max_height_units.map(|v| Box::new(Val::Float64(v)))),
        ),
        (
            "aspect-hint".into(),
            Val::Option(item.aspect_hint.map(|v| Box::new(Val::Float64(v)))),
        ),
        ("priority".into(), Val::S32(item.priority)),
    ])
}

fn spec_val(spec: &LayoutSpec) -> Val {
    Val::Record(vec![
        (
            "columns".into(),
            Val::List(spec.columns.iter().map(track_val).collect()),
        ),
        ("rows".into(), Val::List(spec.rows.iter().map(track_val).collect())),
        ("items".into(), Val::List(spec.items.iter().map(item_val).collect())),
    ])
}

fn parse_descriptor(value: Option<Val>) -> Result<StrategyDescriptor, LayoutFallback> {
    let Val::Record(fields) = value.ok_or_else(|| {
        LayoutFallback::PluginError("describe omitted its result".into())
    })? else {
        return Err(LayoutFallback::PluginError("describe returned a non-record".into()));
    };
    let mut fields: std::collections::BTreeMap<_, _> = fields.into_iter().collect();
    let id = take_string(&mut fields, "id")?;
    let version = take_string(&mut fields, "version")?;
    let description = take_string(&mut fields, "description")?;
    if !fields.is_empty() {
        return Err(LayoutFallback::PluginError(
            "describe returned unknown fields".into(),
        ));
    }
    Ok(StrategyDescriptor { id, version, description })
}

fn take_string(
    fields: &mut std::collections::BTreeMap<String, Val>,
    key: &str,
) -> Result<String, LayoutFallback> {
    match fields.remove(key) {
        Some(Val::String(value)) => Ok(value),
        _ => Err(LayoutFallback::PluginError(format!(
            "missing or non-string {key}"
        ))),
    }
}

fn parse_solution(value: Val) -> Result<LayoutSolution, LayoutFallback> {
    let Val::Record(fields) = value else {
        return Err(LayoutFallback::PluginError(
            "solve success was not a record".into(),
        ));
    };
    let mut fields: std::collections::BTreeMap<String, Val> = fields.into_iter().collect();
    let columns = take_u32(&mut fields, "columns")?;
    let rows = take_u32(&mut fields, "rows")?;
    let rows_per_unit = take_nested_f64(&mut fields, "metrics", "rows-per-unit")?;
    let rects = take_rects(&mut fields, "rects")?;
    let column_track_sizes = take_f64_list(&mut fields, "column-track-sizes")?;
    let row_track_sizes = take_f64_list(&mut fields, "row-track-sizes")?;
    let dropped = take_dropped(&mut fields, "dropped")?;
    let overflow_units_x = take_record_f64(&mut fields, "overflow-units-x")?;
    let overflow_units_y = take_record_f64(&mut fields, "overflow-units-y")?;
    let trailing_gap_units_x = take_record_f64(&mut fields, "trailing-gap-units-x")?;
    let trailing_gap_units_y = take_record_f64(&mut fields, "trailing-gap-units-y")?;
    if !fields.is_empty() {
        return Err(LayoutFallback::PluginError(
            "solve returned unknown fields".into(),
        ));
    }
    // The contract, enforced host-side too: never a rect outside the
    // terminal, never a negative extent. The guest re-validates in its
    // codec; this is the last line before the TUI trusts the plugin.
    for (id, rect) in &rects {
        if rect.width < 0 || rect.height < 0
            || rect.col < 0
            || rect.row < 0
            || rect.col as i64 + rect.width as i64 > columns as i64
            || rect.row as i64 + rect.height as i64 > rows as i64
        {
            return Err(LayoutFallback::PluginError(format!(
                "rect for {id} escapes the terminal: {rect:?}"
            )));
        }
    }
    Ok(LayoutSolution {
        columns,
        rows,
        rows_per_unit,
        rects,
        column_track_sizes,
        row_track_sizes,
        dropped,
        overflow_units_x,
        overflow_units_y,
        trailing_gap_units_x,
        trailing_gap_units_y,
    })
}

fn take_u32(
    fields: &mut std::collections::BTreeMap<String, Val>,
    key: &str,
) -> Result<u32, LayoutFallback> {
    match fields.remove(key) {
        Some(Val::U32(value)) => Ok(value),
        _ => Err(LayoutFallback::PluginError(format!(
            "missing or non-u32 {key}"
        ))),
    }
}

fn take_record_f64(
    fields: &mut std::collections::BTreeMap<String, Val>,
    key: &str,
) -> Result<f64, LayoutFallback> {
    match fields.remove(key) {
        Some(Val::Float64(value)) => Ok(value),
        _ => Err(LayoutFallback::PluginError(format!(
            "missing or non-f64 {key}"
        ))),
    }
}

fn take_record_s32(
    fields: &mut std::collections::BTreeMap<String, Val>,
    key: &str,
) -> Result<i32, LayoutFallback> {
    match fields.remove(key) {
        Some(Val::S32(value)) => Ok(value),
        _ => Err(LayoutFallback::PluginError(format!(
            "missing or non-s32 {key}"
        ))),
    }
}

fn take_nested_f64(
    fields: &mut std::collections::BTreeMap<String, Val>,
    outer: &str,
    inner: &str,
) -> Result<f64, LayoutFallback> {
    match fields.remove(outer) {
        Some(Val::Record(inner_fields)) => {
            let mut inner_fields: std::collections::BTreeMap<String, Val> =
                inner_fields.into_iter().collect();
            take_record_f64(&mut inner_fields, inner)
        }
        _ => Err(LayoutFallback::PluginError(format!(
            "missing or non-record {outer}"
        ))),
    }
}

fn take_f64_list(
    fields: &mut std::collections::BTreeMap<String, Val>,
    key: &str,
) -> Result<Vec<f64>, LayoutFallback> {
    match fields.remove(key) {
        Some(Val::List(values)) => values
            .into_iter()
            .map(|v| match v {
                Val::Float64(value) => Ok(value),
                _ => Err(LayoutFallback::PluginError(format!(
                    "non-f64 entry in {key}"
                ))),
            })
            .collect(),
        _ => Err(LayoutFallback::PluginError(format!(
            "missing or non-list {key}"
        ))),
    }
}

fn take_rects(
    fields: &mut std::collections::BTreeMap<String, Val>,
    key: &str,
) -> Result<Vec<(String, CellRect)>, LayoutFallback> {
    let Val::List(entries) = fields
        .remove(key)
        .ok_or_else(|| LayoutFallback::PluginError(format!("missing {key}")))?
    else {
        return Err(LayoutFallback::PluginError(format!("non-list {key}")));
    };
    let mut rects = Vec::with_capacity(entries.len());
    for entry in entries {
        let Val::Record(fields) = entry else {
            return Err(LayoutFallback::PluginError(format!("non-record entry in {key}")));
        };
        let mut fields: std::collections::BTreeMap<String, Val> =
            fields.into_iter().collect();
        let id = match fields.remove("id") {
            Some(Val::String(value)) => value,
            _ => return Err(LayoutFallback::PluginError(format!("entry in {key} missing id"))),
        };
        let rect_val = fields
            .remove("rect")
            .ok_or_else(|| LayoutFallback::PluginError(format!("entry in {key} missing rect")))?;
        let Val::Record(rect_fields) = rect_val else {
            return Err(LayoutFallback::PluginError(format!("non-record rect in {key}")));
        };
        let mut rect_fields: std::collections::BTreeMap<String, Val> =
            rect_fields.into_iter().collect();
        let col = take_record_s32(&mut rect_fields, "col")?;
        let row = take_record_s32(&mut rect_fields, "row")?;
        let width = take_record_s32(&mut rect_fields, "width")?;
        let height = take_record_s32(&mut rect_fields, "height")?;
        rects.push((
            id,
            CellRect {
                col,
                row,
                width,
                height,
            },
        ));
    }
    Ok(rects)
}

fn take_dropped(
    fields: &mut std::collections::BTreeMap<String, Val>,
    key: &str,
) -> Result<Vec<LayoutDrop>, LayoutFallback> {
    let Val::List(entries) = fields
        .remove(key)
        .ok_or_else(|| LayoutFallback::PluginError(format!("missing {key}")))?
    else {
        return Err(LayoutFallback::PluginError(format!("non-list {key}")));
    };
    let mut dropped = Vec::with_capacity(entries.len());
    for entry in entries {
        let Val::Record(fields) = entry else {
            return Err(LayoutFallback::PluginError(format!("non-record entry in {key}")));
        };
        let mut fields: std::collections::BTreeMap<String, Val> =
            fields.into_iter().collect();
        let id = match fields.remove("id") {
            Some(Val::String(value)) => value,
            _ => return Err(LayoutFallback::PluginError(format!("entry in {key} missing id"))),
        };
        let reason = match fields.remove("reason") {
            Some(Val::String(value)) => value,
            _ => {
                return Err(LayoutFallback::PluginError(format!(
                    "entry in {key} missing reason"
                )))
            }
        };
        dropped.push(LayoutDrop { id, reason });
    }
    Ok(dropped)
}

// ---------------------------------------------------------------------------
// The bridge the TUI imports.
// ---------------------------------------------------------------------------

/// Answers `tina:layout/bridge` for the TUI guest: routes to the mounted
/// strategy plugin, or reports why it cannot. Shared through the linker
/// as an `Arc`; cheap to clone per instance closure.
#[derive(Clone, Default)]
pub struct LayoutBridge {
    runtime: Option<Arc<LayoutPluginRuntime>>,
}

impl LayoutBridge {
    pub fn new(runtime: Option<Arc<LayoutPluginRuntime>>) -> Self {
        Self { runtime }
    }

    pub fn describe(&self) -> Option<StrategyDescriptor> {
        let runtime = self.runtime.as_ref()?;
        match runtime.describe() {
            Ok(descriptor) => Some(descriptor),
            Err(_) => None,
        }
    }

    pub fn solve(&self, request: &LayoutRequest) -> Result<LayoutSolution, LayoutFallback> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(LayoutFallback::NoPlugin)?;
        runtime.solve(request)
    }

    /// The fallback value marshalling shares: map a fallback to its WIT
    /// variant for the guest.
    pub fn fallback_val(fallback: &LayoutFallback) -> Val {
        match fallback {
            LayoutFallback::NoPlugin => Val::Variant("no-plugin".into(), None),
            LayoutFallback::PluginError(message) => Val::Variant(
                "plugin-error".into(),
                Some(Box::new(Val::String(message.clone()))),
            ),
            LayoutFallback::PluginTimeout(message) => Val::Variant(
                "plugin-timeout".into(),
                Some(Box::new(Val::String(message.clone()))),
            ),
        }
    }

    /// The success value: the solution record.
    pub fn solution_val(solution: &LayoutSolution) -> Val {
        Val::Record(vec![
            ("columns".into(), Val::U32(solution.columns)),
            ("rows".into(), Val::U32(solution.rows)),
            (
                "metrics".into(),
                Val::Record(vec![(
                    "rows-per-unit".into(),
                    Val::Float64(solution.rows_per_unit),
                )]),
            ),
            (
                "rects".into(),
                Val::List(
                    solution
                        .rects
                        .iter()
                        .map(|(id, rect)| {
                            Val::Record(vec![
                                ("id".into(), Val::String(id.clone())),
                                (
                                    "rect".into(),
                                    Val::Record(vec![
                                        ("col".into(), Val::S32(rect.col)),
                                        ("row".into(), Val::S32(rect.row)),
                                        ("width".into(), Val::S32(rect.width)),
                                        ("height".into(), Val::S32(rect.height)),
                                    ]),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "column-track-sizes".into(),
                Val::List(
                    solution
                        .column_track_sizes
                        .iter()
                        .map(|v| Val::Float64(*v))
                        .collect(),
                ),
            ),
            (
                "row-track-sizes".into(),
                Val::List(
                    solution
                        .row_track_sizes
                        .iter()
                        .map(|v| Val::Float64(*v))
                        .collect(),
                ),
            ),
            (
                "dropped".into(),
                Val::List(
                    solution
                        .dropped
                        .iter()
                        .map(|d| {
                            Val::Record(vec![
                                ("id".into(), Val::String(d.id.clone())),
                                ("reason".into(), Val::String(d.reason.clone())),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("overflow-units-x".into(), Val::Float64(solution.overflow_units_x)),
            ("overflow-units-y".into(), Val::Float64(solution.overflow_units_y)),
            (
                "trailing-gap-units-x".into(),
                Val::Float64(solution.trailing_gap_units_x),
            ),
            (
                "trailing-gap-units-y".into(),
                Val::Float64(solution.trailing_gap_units_y),
            ),
        ])
    }
}

/// Parse a `solve` request from the guest's Val record (the bridge's
/// host-side marshalling; also used by the parity tests).
pub fn parse_request(
    spec: &Val,
    columns: &Val,
    rows: &Val,
    metrics: &Val,
) -> Result<LayoutRequest, LayoutFallback> {
    let Val::Record(spec_fields) = spec else {
        return Err(LayoutFallback::PluginError("spec is not a record".into()));
    };
    let mut spec_fields: std::collections::BTreeMap<String, Val> =
        spec_fields.clone().into_iter().collect();
    let spec_columns = take_tracks(&mut spec_fields, "columns")?;
    let spec_rows = take_tracks(&mut spec_fields, "rows")?;
    let items = take_items(&mut spec_fields, "items")?;
    if !spec_fields.is_empty() {
        return Err(LayoutFallback::PluginError("spec has unknown fields".into()));
    }
    let Val::U32(columns) = *columns else {
        return Err(LayoutFallback::PluginError("columns is not a u32".into()));
    };
    let Val::U32(rows) = *rows else {
        return Err(LayoutFallback::PluginError("rows is not a u32".into()));
    };
    let Val::Record(metrics_fields) = metrics else {
        return Err(LayoutFallback::PluginError("metrics is not a record".into()));
    };
    let mut metrics_fields: std::collections::BTreeMap<String, Val> =
        metrics_fields.clone().into_iter().collect();
    let rows_per_unit = take_record_f64(&mut metrics_fields, "rows-per-unit")?;
    if !metrics_fields.is_empty() {
        return Err(LayoutFallback::PluginError(
            "metrics has unknown fields".into(),
        ));
    }
    Ok(LayoutRequest {
        spec: LayoutSpec {
            columns: spec_columns,
            rows: spec_rows,
            items,
        },
        columns,
        rows,
        metrics: CellMetrics { rows_per_unit },
    })
}

fn take_tracks(
    fields: &mut std::collections::BTreeMap<String, Val>,
    key: &str,
) -> Result<Vec<LayoutTrack>, LayoutFallback> {
    let Val::List(tracks) = fields
        .remove(key)
        .ok_or_else(|| LayoutFallback::PluginError(format!("missing {key}")))?
    else {
        return Err(LayoutFallback::PluginError(format!("non-list {key}")));
    };
    let mut out = Vec::with_capacity(tracks.len());
    for track in tracks {
        let Val::Record(fields) = track else {
            return Err(LayoutFallback::PluginError(format!("non-record track in {key}")));
        };
        let mut fields: std::collections::BTreeMap<String, Val> =
            fields.into_iter().collect();
        let kind = match fields.remove("kind") {
            Some(Val::Enum(kind)) => match kind.as_str() {
                "fixed" => LayoutTrackKind::Fixed,
                "weighted" => LayoutTrackKind::Weighted,
                "auto" => LayoutTrackKind::Auto,
                _ => {
                    return Err(LayoutFallback::PluginError(format!(
                        "unknown track kind {kind}"
                    )))
                }
            },
            _ => {
                return Err(LayoutFallback::PluginError(format!(
                    "track in {key} missing kind"
                )))
            }
        };
        let size_units = take_record_f64(&mut fields, "size-units")?;
        let weight = take_record_f64(&mut fields, "weight")?;
        let min_units = take_record_f64(&mut fields, "min-units")?;
        let max_units = match fields.remove("max-units") {
            Some(Val::Option(v)) => v.map(|boxed| match *boxed {
                Val::Float64(v) => v,
                _ => f64::NAN,
            }),
            _ => {
                return Err(LayoutFallback::PluginError(format!(
                    "track in {key} missing max-units"
                )))
            }
        };
        out.push(LayoutTrack {
            kind,
            size_units,
            weight,
            min_units,
            max_units,
        });
    }
    Ok(out)
}

fn take_items(
    fields: &mut std::collections::BTreeMap<String, Val>,
    key: &str,
) -> Result<Vec<LayoutItem>, LayoutFallback> {
    let Val::List(items) = fields
        .remove(key)
        .ok_or_else(|| LayoutFallback::PluginError(format!("missing {key}")))?
    else {
        return Err(LayoutFallback::PluginError(format!("non-list {key}")));
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Val::Record(fields) = item else {
            return Err(LayoutFallback::PluginError(format!("non-record item in {key}")));
        };
        let mut item_fields: std::collections::BTreeMap<String, Val> =
            fields.into_iter().collect();
        let mut column = LayoutSpan { start: None, span: 1 };
        let mut row = LayoutSpan { start: None, span: 1 };
        let mut id = String::new();
        let mut min_width_units = 0.0;
        let mut min_height_units = 0.0;
        let mut max_width_units = None;
        let mut max_height_units = None;
        let mut aspect_hint = None;
        let mut priority = 0i32;
        // Sorted field order: aspect-hint, column, id, max-height-units,
        // max-width-units, min-height-units, min-width-units, priority,
        // row. Sort-and-match keeps this independent of guest order.
        while let Some((name, value)) = item_fields.pop_first() {
            match (name.as_str(), value) {
                ("id", Val::String(v)) => id = v,
                ("column", v) | ("row", v) => {
                    let span = parse_span(v)?;
                    if name == "column" {
                        column = span;
                    } else {
                        row = span;
                    }
                }
                ("min-width-units", Val::Float64(v)) => min_width_units = v,
                ("min-height-units", Val::Float64(v)) => min_height_units = v,
                ("max-width-units", Val::Option(v)) => {
                    max_width_units = v.map(|b| match *b {
                        Val::Float64(v) => v,
                        _ => f64::NAN,
                    })
                }
                ("max-height-units", Val::Option(v)) => {
                    max_height_units = v.map(|b| match *b {
                        Val::Float64(v) => v,
                        _ => f64::NAN,
                    })
                }
                ("aspect-hint", Val::Option(v)) => {
                    aspect_hint = v.map(|b| match *b {
                        Val::Float64(v) => v,
                        _ => f64::NAN,
                    })
                }
                ("priority", Val::S32(v)) => priority = v,
                _ => {
                    return Err(LayoutFallback::PluginError(format!(
                        "item in {key} has a bad field {name}"
                    )))
                }
            }
        }
        out.push(LayoutItem {
            id,
            column,
            row,
            min_width_units,
            min_height_units,
            max_width_units,
            max_height_units,
            aspect_hint,
            priority,
        });
    }
    Ok(out)
}

fn parse_span(value: Val) -> Result<LayoutSpan, LayoutFallback> {
    let Val::Record(fields) = value else {
        return Err(LayoutFallback::PluginError("span is not a record".into()));
    };
    let mut fields: std::collections::BTreeMap<String, Val> = fields.into_iter().collect();
    let start = match fields.remove("start") {
        Some(Val::Option(v)) => v.map(|boxed| match *boxed {
            Val::U32(v) => v,
            _ => u32::MAX,
        }),
        _ => {
            return Err(LayoutFallback::PluginError(
                "span missing start".into(),
            ))
        }
    };
    let span = match fields.remove("span") {
        Some(Val::U32(v)) => v,
        _ => return Err(LayoutFallback::PluginError("span missing span".into())),
    };
    Ok(LayoutSpan { start, span })
}

/// The epoch ticker (same technique as `plugin_loader`; duplicated here
/// because that one is private to its module).
struct EpochTicker {
    stop: Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl EpochTicker {
    fn spawn(engine: &Engine, tick: Duration) -> Result<Self> {
        use std::sync::atomic::{AtomicBool, Ordering};
        let stop = Arc::new(AtomicBool::new(false));
        let engine = engine.clone();
        let flag = stop.clone();
        let worker = std::thread::Builder::new()
            .name("layout-epoch-ticker".into())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    std::thread::sleep(tick);
                    engine.increment_epoch();
                }
            })
            .map_err(|error| {
                wasmtime::Error::msg(format!("layout epoch ticker spawn failed: {error}"))
            })?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Layout budget defaults (tighter than engine plugins).
pub fn layout_limits(call_deadline: Duration) -> PluginLimits {
    PluginLimits {
        call_deadline,
        instantiation_deadline: DEFAULT_INSTANTIATION_DEADLINE,
        epoch_tick: DEFAULT_EPOCH_TICK,
        memory_bytes: crate::plugin_loader::DEFAULT_MEMORY_LIMIT_BYTES,
        table_elements: crate::plugin_loader::DEFAULT_TABLE_ELEMENTS,
    }
}

pub use crate::plugin_loader::{DEFAULT_MEMORY_LIMIT_BYTES, DEFAULT_TABLE_ELEMENTS};
#[cfg(test)]
mod tests {
    use super::*;

    fn solver_component() -> Vec<u8> {
        include_bytes!("../testdata/layout/layout_solver.component.wasm").to_vec()
    }

    fn hang_component() -> Vec<u8> {
        include_bytes!("../testdata/layout/layout_hang.component.wasm").to_vec()
    }

    fn chat_request() -> LayoutRequest {
        // The chat screen's outer grid, in boundary-record form.
        let track = |kind, weight, min_units| LayoutTrack {
            kind,
            size_units: 0.0,
            weight,
            min_units,
            max_units: None,
        };
        LayoutRequest {
            spec: LayoutSpec {
                columns: vec![
                    track(LayoutTrackKind::Weighted, 65.0, 30.0),
                    track(LayoutTrackKind::Weighted, 35.0, 20.0),
                ],
                rows: vec![track(LayoutTrackKind::Weighted, 1.0, 0.0)],
                items: vec![
                    LayoutItem {
                        id: "chat".into(),
                        column: LayoutSpan { start: Some(0), span: 1 },
                        row: LayoutSpan { start: Some(0), span: 1 },
                        min_width_units: 0.0,
                        min_height_units: 0.0,
                        max_width_units: None,
                        max_height_units: None,
                        aspect_hint: None,
                        priority: 0,
                    },
                    LayoutItem {
                        id: "info".into(),
                        column: LayoutSpan { start: Some(1), span: 1 },
                        row: LayoutSpan { start: Some(0), span: 1 },
                        min_width_units: 0.0,
                        min_height_units: 0.0,
                        max_width_units: None,
                        max_height_units: None,
                        aspect_hint: None,
                        priority: 0,
                    },
                ],
            },
            columns: 100,
            rows: 24,
            metrics: CellMetrics { rows_per_unit: 2.0 },
        }
    }

    #[test]
    fn solver_plugin_describes_the_contract() {
        let runtime = LayoutPluginRuntime::new(
            solver_component(),
            None,
            layout_limits(DEFAULT_LAYOUT_CALL_DEADLINE),
        )
        .unwrap();
        let descriptor = runtime.describe().expect("describe answers");
        assert_eq!(descriptor.id, "tina/layout-solver");
        assert_eq!(descriptor.version, "1.0.0");
    }

    #[test]
    fn solver_plugin_matches_the_golden_table() {
        // The committed golden table is generated by tina's Dart side
        // (LayoutWire.goldenCase for every battery spec); the plugin must
        // agree on every entry through the real Store + Val boundary.
        let table = serde_json::from_str::<GoldenTable>(include_str!(
            "../testdata/layout/goldens.json"
        ))
        .unwrap();
        assert!(!table.cases.is_empty(), "the golden table is not empty");
        let runtime = LayoutPluginRuntime::new(
            solver_component(),
            None,
            layout_limits(Duration::from_secs(10)),
        )
        .unwrap();
        for case in &table.cases {
            let request = case.request.layout_request();
            let solution = runtime.solve(&request).unwrap_or_else(|e| {
                panic!("case {} failed: {e:?}", case.name)
            });
            let expected = case.expected.layout_solution();
            assert_eq!(solution.rects, expected.rects, "case {}", case.name);
            assert_eq!(
                solution.column_track_sizes, expected.column_track_sizes,
                "case {}",
                case.name
            );
            assert_eq!(
                solution.row_track_sizes, expected.row_track_sizes,
                "case {}",
                case.name
            );
            assert_eq!(solution.rows_per_unit, expected.rows_per_unit, "case {}", case.name);
        }
    }

    #[test]
    fn a_wedged_strategy_is_epoch_killed_and_never_wedges_the_host() {
        let runtime = LayoutPluginRuntime::new(
            hang_component(),
            None,
            layout_limits(Duration::from_millis(150)),
        )
        .unwrap();
        let descriptor = runtime.describe().expect("describe still answers");
        assert_eq!(descriptor.id, "tina/layout-hang");
        let started = std::time::Instant::now();
        let failure = runtime.solve(&chat_request()).expect_err("solve must fail");
        match failure {
            LayoutFallback::PluginTimeout(_) => {}
            other => panic!("expected a timeout, got {other:?}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the kill must be prompt"
        );
        // A second solve retries on a fresh Store and fails the same way.
        let again = runtime.solve(&chat_request()).expect_err("second solve fails");
        assert!(matches!(again, LayoutFallback::PluginTimeout(_)));
    }

    #[test]
    fn the_focus_strategy_disagrees_through_the_real_boundary() {
        // The two published artifacts, through their own Stores: the same
        // chat-split request yields 65/35 from tina/layout-solver and
        // 50/50 from tina/layout-focus. This is the swap demo at the
        // boundary the TUI consumes.
        let solver = LayoutPluginRuntime::new(
            solver_component(),
            None,
            layout_limits(Duration::from_secs(10)),
        )
        .unwrap();
        let focus = LayoutPluginRuntime::new(
            include_bytes!("../testdata/layout/layout_focus.component.wasm").to_vec(),
            None,
            layout_limits(Duration::from_secs(10)),
        )
        .unwrap();
        assert_eq!(focus.describe().unwrap().id, "tina/layout-focus");
        let request = chat_request();
        let via_solver = solver.solve(&request).expect("solver answers");
        let via_focus = focus.solve(&request).expect("focus answers");
        let chat_width = |s: &LayoutSolution| {
            s.rects.iter().find(|(id, _)| id == "chat").unwrap().1.width
        };
        assert_eq!(chat_width(&via_solver), 65, "65% split");
        assert_eq!(chat_width(&via_focus), 50, "equal-share focus");
    }

    #[test]
    fn solve_latency_stays_far_below_a_frame_budget() {
        let runtime = LayoutPluginRuntime::new(
            solver_component(),
            None,
            layout_limits(DEFAULT_LAYOUT_CALL_DEADLINE),
        )
        .unwrap();
        let request = chat_request();
        // Warm the Store (first call instantiates).
        runtime.solve(&request).expect("warm-up solve");
        let rounds = 50;
        let started = std::time::Instant::now();
        for _ in 0..rounds {
            runtime.solve(&request).expect("solve");
        }
        let per_call = started.elapsed() / rounds;
        eprintln!(
            "layout solve through the host boundary: {:?}/call ({rounds} rounds)",
            per_call
        );
        // Debug-build hosts measure ~26ms/call (the dynamic Val lowering is
        // unoptimized there); release ~0.8ms. The assert accommodates the
        // dev profile while still catching pathological regressions; the
        // recorded numbers live in the layout-plugin report.
        assert!(
            per_call < Duration::from_millis(50),
            "solve latency {per_call:?} must stay below even a debug \
             host's frame budget"
        );
    }

    #[test]
    fn zero_capability_admission_refuses_foreign_imports() {
        // The wasm-prompt plugin imports tina:plugin-host/session: it must
        // be refused as a layout strategy even though it is a valid
        // component.
        let bytes = include_bytes!("../testdata/plugin_probe/tina_prompt_plugin.component.wasm");
        let runtime = LayoutPluginRuntime::new(
            bytes.to_vec(),
            None,
            layout_limits(DEFAULT_LAYOUT_CALL_DEADLINE),
        )
        .expect("construction is lazy; admission happens at load");
        // The wasm-prompt plugin imports tina:plugin-host/session: the
        // first call must refuse it with the zero-capability message.
        let error = format!("{:?}", runtime.describe().expect_err("admission refuses"));
        assert!(error.contains("forbidden"), "got: {error}");
    }

    // -- golden-table JSON shapes (mirror tina's LayoutWire) ------------

    #[derive(serde::Deserialize)]
    struct GoldenTable {
        cases: Vec<GoldenCase>,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct GoldenCase {
        name: String,
        request: WireRequest,
        expected: WireSolution,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct WireRequest {
        spec: WireSpec,
        columns: u32,
        rows: u32,
        metrics: WireMetrics,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct WireSpec {
        columns: Vec<WireTrack>,
        rows: Vec<WireTrack>,
        items: Vec<WireItem>,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct WireTrack {
        kind: String,
        size_units: f64,
        weight: f64,
        min_units: f64,
        max_units: Option<f64>,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct WireItem {
        id: String,
        column: WireSpan,
        row: WireSpan,
        min_width_units: f64,
        min_height_units: f64,
        max_width_units: Option<f64>,
        max_height_units: Option<f64>,
        aspect_hint: Option<f64>,
        priority: i32,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct WireSpan {
        start: Option<u32>,
        span: u32,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct WireMetrics {
        rows_per_unit: f64,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct WireSolution {
        columns: u32,
        rows: u32,
        metrics: WireMetrics,
        rects: Vec<WireRectEntry>,
        column_track_sizes: Vec<f64>,
        row_track_sizes: Vec<f64>,
        dropped: Vec<WireDrop>,
        overflow_units_x: f64,
        overflow_units_y: f64,
        trailing_gap_units_x: f64,
        trailing_gap_units_y: f64,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct WireRectEntry {
        id: String,
        rect: WireRect,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct WireRect {
        col: i32,
        row: i32,
        width: i32,
        height: i32,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct WireDrop {
        id: String,
        reason: String,
    }

    impl WireRequest {
        fn layout_request(&self) -> LayoutRequest {
            LayoutRequest {
                spec: self.spec.layout_spec(),
                columns: self.columns,
                rows: self.rows,
                metrics: CellMetrics {
                    rows_per_unit: self.metrics.rows_per_unit,
                },
            }
        }
    }

    impl WireSpec {
        fn layout_spec(&self) -> LayoutSpec {
            LayoutSpec {
                columns: self.columns.iter().map(|t| t.layout_track()).collect(),
                rows: self.rows.iter().map(|t| t.layout_track()).collect(),
                items: self.items.iter().map(|i| i.layout_item()).collect(),
            }
        }
    }

    impl WireTrack {
        fn layout_track(&self) -> LayoutTrack {
            LayoutTrack {
                kind: match self.kind.as_str() {
                    "fixed" => LayoutTrackKind::Fixed,
                    "weighted" => LayoutTrackKind::Weighted,
                    _ => LayoutTrackKind::Auto,
                },
                size_units: self.size_units,
                weight: self.weight,
                min_units: self.min_units,
                max_units: self.max_units,
            }
        }
    }

    impl WireItem {
        fn layout_item(&self) -> LayoutItem {
            LayoutItem {
                id: self.id.clone(),
                column: self.column.layout_span(),
                row: self.row.layout_span(),
                min_width_units: self.min_width_units,
                min_height_units: self.min_height_units,
                max_width_units: self.max_width_units,
                max_height_units: self.max_height_units,
                aspect_hint: self.aspect_hint,
                priority: self.priority,
            }
        }
    }

    impl WireSpan {
        fn layout_span(&self) -> LayoutSpan {
            LayoutSpan {
                start: self.start,
                span: self.span,
            }
        }
    }

    impl WireSolution {
        fn layout_solution(&self) -> LayoutSolution {
            LayoutSolution {
                columns: self.columns,
                rows: self.rows,
                rows_per_unit: self.metrics.rows_per_unit,
                rects: self
                    .rects
                    .iter()
                    .map(|e| {
                        (
                            e.id.clone(),
                            CellRect {
                                col: e.rect.col,
                                row: e.rect.row,
                                width: e.rect.width,
                                height: e.rect.height,
                            },
                        )
                    })
                    .collect(),
                column_track_sizes: self.column_track_sizes.clone(),
                row_track_sizes: self.row_track_sizes.clone(),
                dropped: self
                    .dropped
                    .iter()
                    .map(|d| LayoutDrop {
                        id: d.id.clone(),
                        reason: d.reason.clone(),
                    })
                    .collect(),
                overflow_units_x: self.overflow_units_x,
                overflow_units_y: self.overflow_units_y,
                trailing_gap_units_x: self.trailing_gap_units_x,
                trailing_gap_units_y: self.trailing_gap_units_y,
            }
        }
    }
}
