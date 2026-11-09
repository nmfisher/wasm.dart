//! Narrow v0.1.0 component loader used by the plugin boundary proof.
//!
//! Every plugin owns a Store. Only host-created proxy resources live in that
//! Store; service calls carry scalar data through a trusted host broker. This
//! synchronous probe has no fuel/epoch cancellation, memory budget, or nested
//! same-Store reentry, so it is not yet a production untrusted-code sandbox.
use std::{
    collections::{BTreeMap, HashMap},
    sync::atomic::{AtomicU64, Ordering},
};

use wasmtime::component::types::{ComponentFunc, Type};
use wasmtime::{
    Engine, Store,
    component::{
        Component, Func, Linker, Resource, ResourceAny, ResourceType, Val, types::ComponentItem,
    },
};
use wasmtime::{Result, error::Context as _};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView, p3};

const ABI_SECTION: &[u8] = b"tina.embedder-abi";
pub const EMBEDDER_ABI_VERSION: u32 = 1;
pub const PLUGIN_WORLD_VERSION: &str = "tina:plugin-probe/plugin@0.1.0";
const ABI_VERSION: u32 = EMBEDDER_ABI_VERSION;
const SERVICE_TYPES: &str = "tina:plugin-probe/service-types@0.1.0";
const ENGINE_SERVICE: &str = "tina:plugin-probe/engine-service@0.1.0";
const PLUGIN_INTERFACE: &str = PLUGIN_WORLD_VERSION;

static NEXT_LOADER_ID: AtomicU64 = AtomicU64::new(1);

/// Opaque loader identity, not an authorization token. The boundary compares
/// all three fields before looking up any Store-owned Wasmtime handle.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PluginId {
    loader: u64,
    slot: u64,
    generation: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct ServiceCaller {
    plugin: PluginId,
    call: u64,
}

impl ServiceCaller {
    pub fn plugin(&self) -> PluginId {
        self.plugin
    }
    pub fn call_number(&self) -> u64 {
        self.call
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceCallError {
    Denied,
    Unavailable,
}

/// Immutable, host-captured call data. It is copied into the plugin Store for
/// one invocation and is never accepted from the plugin request itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginCallFrame {
    pub session_id: String,
    pub model: String,
}

/// Trusted host service implementation. Caller identity, frame and call scope
/// are supplied by the loader, never taken from guest request fields.
pub trait PluginServiceBroker: Send + Sync + 'static {
    fn call(
        &self,
        caller: ServiceCaller,
        request: &str,
        frame: Option<&PluginCallFrame>,
    ) -> std::result::Result<String, ServiceCallError>;
}

/// Host policy. Interface admission and call authority are separate grants.
#[derive(Clone, Copy, Debug, Default)]
pub struct PluginPolicy {
    pub allow_engine_service_import: bool,
    pub allow_engine_service_call: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginDescriptor {
    pub id: String,
    pub description: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PluginFailure {
    Rejected(String),
    Failed(String),
    ServiceDenied,
    ServiceUnavailable,
}

#[derive(Clone, Copy)]
struct Proxy {
    owner: PluginId,
}

struct PluginStore {
    wasi: WasiCtx,
    table: ResourceTable,
    handle: PluginId,
    allow_call: bool,
    active_call: Option<u64>,
    active_frame: Option<PluginCallFrame>,
    next_call: u64,
}

impl WasiView for PluginStore {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

struct ActivePlugin {
    handle: PluginId,
    store: Store<PluginStore>,
    run: Func,
    describe: Func,
    proxy: ResourceAny,
    proxy_rep: u32,
}

/// Owns independent Wasmtime Stores. Public handles carry loader identity,
/// slot and checked generation; Wasmtime functions/resources never escape.
pub struct PluginBoundary {
    engine: Engine,
    broker: std::sync::Arc<dyn PluginServiceBroker>,
    loader_id: u64,
    next_slot: u64,
    next_generation: u64,
    free_slots: Vec<u64>,
    plugins: HashMap<u64, ActivePlugin>,
}

impl PluginBoundary {
    pub fn new(engine: Engine, broker: std::sync::Arc<dyn PluginServiceBroker>) -> Result<Self> {
        let loader_id = NEXT_LOADER_ID
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| wasmtime::Error::msg("plugin loader identity exhausted"))?;
        Ok(Self {
            engine,
            broker,
            loader_id,
            next_slot: 1,
            next_generation: 1,
            free_slots: Vec::new(),
            plugins: HashMap::new(),
        })
    }

    pub fn load(&mut self, bytes: &[u8], policy: PluginPolicy) -> Result<PluginId> {
        verify_embedder_abi(bytes)?;
        let component =
            Component::new(&self.engine, bytes).context("compiling plugin component")?;
        validate_probe_component(&self.engine, &component, policy)?;
        let reuse_slot = self.free_slots.last().copied();
        let slot = reuse_slot.unwrap_or(self.next_slot);
        let next_slot = if reuse_slot.is_none() {
            Some(
                self.next_slot
                    .checked_add(1)
                    .ok_or_else(|| wasmtime::Error::msg("plugin slot id exhausted"))?,
            )
        } else {
            None
        };
        let generation = self.next_generation;
        let next_generation = generation
            .checked_add(1)
            .ok_or_else(|| wasmtime::Error::msg("plugin generation exhausted"))?;
        wasmtime::ensure!(
            !self.plugins.contains_key(&slot),
            "plugin slot unexpectedly occupied"
        );
        let handle = PluginId {
            loader: self.loader_id,
            slot,
            generation,
        };

        let mut linker = Linker::<PluginStore>::new(&self.engine);
        p3::add_to_linker::<PluginStore>(&mut linker)?;
        {
            let mut root = linker.root();
            let mut trace = root.instance("wasm:dart/trace@1.0.0")?;
            trace.func_wrap("capture-utf16", |_store, (_capacity,): (u32,)| {
                Ok((String::new(),))
            })?;
        }
        {
            let mut root = linker.root();
            let mut service_types = root.instance(SERVICE_TYPES)?;
            service_types.resource("service", ResourceType::host::<Proxy>(), |_store, _rep| {
                Ok(())
            })?;
        }
        {
            let broker = self.broker.clone();
            let mut root = linker.root();
            let mut service = root.instance(ENGINE_SERVICE)?;
            service.func_new("call", move |mut store, _ty, params, results| {
                wasmtime::ensure!(
                    params.len() == 2 && results.len() == 1,
                    "unexpected engine-service.call arity"
                );
                let Val::Resource(resource) = params[0].clone() else {
                    wasmtime::bail!("service argument is not a resource");
                };
                let Val::String(request) = &params[1] else {
                    wasmtime::bail!("request argument is not a string");
                };
                wasmtime::ensure!(
                    !resource.owned() && resource.ty() == ResourceType::host::<Proxy>(),
                    "wrong service proxy type/ownership"
                );
                let data = store.data();
                let call = data.active_call.ok_or_else(|| {
                    wasmtime::Error::msg("service call outside active plugin call")
                })?;
                let expected_owner = data.handle;
                let allowed = data.allow_call;
                let frame = data.active_frame.clone();
                // Converting a dynamic borrow to Resource<Proxy> clears its
                // Wasmtime borrow-table state exactly once.
                let typed: Resource<Proxy> = resource.try_into_resource(&mut store)?;
                wasmtime::ensure!(
                    !typed.owned(),
                    "service proxy borrow unexpectedly became owned"
                );
                let actual_owner = store.data().table.get(&typed)?.owner;
                wasmtime::ensure!(
                    actual_owner == expected_owner,
                    "service proxy belongs to another plugin"
                );
                let caller = ServiceCaller {
                    plugin: expected_owner,
                    call,
                };
                let response = if !allowed {
                    Err(ServiceCallError::Denied)
                } else {
                    broker.call(caller, request, frame.as_ref())
                };
                results[0] = match response {
                    Ok(value) => Val::Result(Ok(Some(Box::new(Val::String(value))))),
                    Err(ServiceCallError::Denied) => {
                        Val::Result(Err(Some(Box::new(Val::Variant("denied".into(), None)))))
                    }
                    Err(ServiceCallError::Unavailable) => Val::Result(Err(Some(Box::new(
                        Val::Variant("unavailable".into(), None),
                    )))),
                };
                Ok(())
            })?;
        }
        let pre = linker
            .instantiate_pre(&component)
            .context("pre-instantiation import type-check")?;
        let mut store = Store::new(
            &self.engine,
            PluginStore {
                wasi: WasiCtxBuilder::new().build(),
                table: ResourceTable::new(),
                handle,
                allow_call: policy.allow_engine_service_call,
                active_call: None,
                active_frame: None,
                next_call: 1,
            },
        );
        let proxy = store.data_mut().table.push(Proxy { owner: handle })?;
        let proxy_rep = proxy.rep();
        let proxy = ResourceAny::try_from_resource(proxy, &mut store)?;
        let instance = pre
            .instantiate(&mut store)
            .context("instantiating plugin component")?;
        let export = component
            .get_export_index(None, PLUGIN_INTERFACE)
            .ok_or_else(|| wasmtime::Error::msg("missing plugin interface export"))?;
        let describe_index = instance
            .get_export_index(&mut store, Some(&export), "describe")
            .ok_or_else(|| wasmtime::Error::msg("missing describe export"))?;
        let run_index = instance
            .get_export_index(&mut store, Some(&export), "run")
            .ok_or_else(|| wasmtime::Error::msg("missing run export"))?;
        let describe = instance
            .get_func(&mut store, &describe_index)
            .ok_or_else(|| wasmtime::Error::msg("describe is not callable"))?;
        let run = instance
            .get_func(&mut store, &run_index)
            .ok_or_else(|| wasmtime::Error::msg("run is not callable"))?;
        if let Some(next_slot) = next_slot {
            self.next_slot = next_slot;
        }
        if reuse_slot.is_some() {
            self.free_slots.pop();
        }
        self.next_generation = next_generation;
        self.plugins.insert(
            slot,
            ActivePlugin {
                handle,
                store,
                run,
                describe,
                proxy,
                proxy_rep,
            },
        );
        Ok(handle)
    }

    pub fn describe(&mut self, handle: PluginId) -> Result<PluginDescriptor> {
        let call = {
            let plugin = self.active_mut(handle)?;
            let mut results = [Val::Record(Vec::new())];
            plugin
                .describe
                .call(&mut plugin.store, &[], &mut results)
                .map(|()| results.into_iter().next())
        };
        let value = match call {
            Ok(Some(value)) => value,
            Ok(None) => wasmtime::bail!("describe omitted its result"),
            Err(error) => {
                let _ = self.retire(handle);
                return Err(error).context("plugin describe trapped; Store retired");
            }
        };
        parse_descriptor(value)
    }

    pub fn run(
        &mut self,
        handle: PluginId,
        input: &str,
    ) -> Result<std::result::Result<String, PluginFailure>> {
        self.run_with_frame(handle, input, None)
    }

    /// Invoke with immutable host-owned call data that is visible only to
    /// broker operations during this call. It does not enter the request text.
    pub fn run_with_frame(
        &mut self,
        handle: PluginId,
        input: &str,
        frame: Option<PluginCallFrame>,
    ) -> Result<std::result::Result<String, PluginFailure>> {
        let plugin = self.active_mut(handle)?;
        wasmtime::ensure!(
            plugin.store.data().active_call.is_none(),
            "plugin call is already active"
        );
        let call = plugin.store.data().next_call;
        plugin.store.data_mut().next_call = call
            .checked_add(1)
            .ok_or_else(|| wasmtime::Error::msg("plugin call id exhausted"))?;
        plugin.store.data_mut().active_call = Some(call);
        plugin.store.data_mut().active_frame = frame;
        let params = [
            Val::Resource(plugin.proxy.clone()),
            Val::String(input.to_owned()),
        ];
        let mut results = [Val::Result(Ok(None))];
        let call_result = plugin.run.call(&mut plugin.store, &params, &mut results);
        plugin.store.data_mut().active_call = None;
        plugin.store.data_mut().active_frame = None;
        if let Err(error) = call_result {
            let _ = self.retire(handle);
            return Err(error).context("plugin run trapped; Store retired");
        }
        parse_run_result(
            results
                .into_iter()
                .next()
                .ok_or_else(|| wasmtime::Error::msg("run omitted its result"))?,
        )
    }

    pub fn retire(&mut self, handle: PluginId) -> Result<()> {
        let plugin = self.remove_active(handle)?;
        let ActivePlugin {
            mut store,
            proxy,
            proxy_rep,
            ..
        } = plugin;
        let cleanup = match proxy.resource_drop(&mut store) {
            Ok(()) => store
                .data_mut()
                .table
                .delete(Resource::<Proxy>::new_own(proxy_rep))
                .map(|_| ())
                .map_err(Into::into),
            Err(error) => Err(error),
        };
        drop(store);
        self.free_slots.push(handle.slot);
        cleanup
    }

    fn active_mut(&mut self, handle: PluginId) -> Result<&mut ActivePlugin> {
        wasmtime::ensure!(
            handle.loader == self.loader_id,
            "plugin handle belongs to another loader"
        );
        let plugin = self
            .plugins
            .get_mut(&handle.slot)
            .ok_or_else(|| wasmtime::Error::msg("plugin is retired or absent"))?;
        wasmtime::ensure!(
            plugin.handle.generation == handle.generation,
            "stale plugin generation"
        );
        Ok(plugin)
    }

    fn remove_active(&mut self, handle: PluginId) -> Result<ActivePlugin> {
        wasmtime::ensure!(
            handle.loader == self.loader_id,
            "plugin handle belongs to another loader"
        );
        let plugin = self
            .plugins
            .get(&handle.slot)
            .ok_or_else(|| wasmtime::Error::msg("plugin is retired or absent"))?;
        wasmtime::ensure!(
            plugin.handle.generation == handle.generation,
            "stale plugin generation"
        );
        Ok(self.plugins.remove(&handle.slot).unwrap())
    }
}

fn parse_descriptor(value: Val) -> Result<PluginDescriptor> {
    let Val::Record(fields) = value else {
        wasmtime::bail!("describe returned a non-record")
    };
    let mut fields: BTreeMap<_, _> = fields.into_iter().collect();
    let id = take_string(&mut fields, "id")?;
    let description = take_string(&mut fields, "description")?;
    wasmtime::ensure!(fields.is_empty(), "describe returned unknown fields");
    Ok(PluginDescriptor { id, description })
}

fn take_string(fields: &mut BTreeMap<String, Val>, key: &str) -> Result<String> {
    match fields.remove(key) {
        Some(Val::String(value)) => Ok(value),
        _ => wasmtime::bail!("missing or non-string {key}"),
    }
}

fn parse_run_result(value: Val) -> Result<std::result::Result<String, PluginFailure>> {
    let Val::Result(result) = value else {
        wasmtime::bail!("run returned a non-result")
    };
    match result {
        Ok(Some(value)) => match *value {
            Val::String(value) => Ok(Ok(value)),
            _ => wasmtime::bail!("run success was not a string"),
        },
        Ok(None) => wasmtime::bail!("run result omitted success payload"),
        Err(Some(value)) => parse_plugin_failure(*value).map(Err),
        Err(None) => wasmtime::bail!("run result omitted error payload"),
    }
}

fn parse_plugin_failure(value: Val) -> Result<PluginFailure> {
    let Val::Variant(name, payload) = value else {
        wasmtime::bail!("plugin error was not a variant")
    };
    match (name.as_str(), payload) {
        ("rejected", Some(value)) => match *value {
            Val::String(s) => Ok(PluginFailure::Rejected(s)),
            _ => wasmtime::bail!("rejected payload was not a string"),
        },
        ("failed", Some(value)) => match *value {
            Val::String(s) => Ok(PluginFailure::Failed(s)),
            _ => wasmtime::bail!("failed payload was not a string"),
        },
        ("service", Some(value)) => match *value {
            Val::Variant(tag, None) if tag == "denied" => Ok(PluginFailure::ServiceDenied),
            Val::Variant(tag, None) if tag == "unavailable" => {
                Ok(PluginFailure::ServiceUnavailable)
            }
            _ => wasmtime::bail!("unknown service failure variant"),
        },
        _ => wasmtime::bail!("unknown plugin failure variant"),
    }
}

/// Validate the outer embedder stamp before passing bytes to Wasmtime.
pub fn verify_embedder_abi(bytes: &[u8]) -> Result<()> {
    const HEADER: &[u8] = b"\0asm\r\0\x01\0";
    wasmtime::ensure!(
        bytes.starts_with(HEADER),
        "invalid component header/version"
    );
    let mut offset = HEADER.len();
    let mut found: Option<&[u8]> = None;
    while offset < bytes.len() {
        let id = bytes[offset];
        offset += 1;
        let length = read_uleb(bytes, &mut offset)? as usize;
        let end = offset
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| wasmtime::Error::msg("section length exceeds file"))?;
        if id == 0 {
            let mut body = offset;
            let name_length = read_uleb(bytes, &mut body)? as usize;
            let name_end = body
                .checked_add(name_length)
                .filter(|name_end| *name_end <= end)
                .ok_or_else(|| wasmtime::Error::msg("custom section name exceeds section"))?;
            if &bytes[body..name_end] == ABI_SECTION {
                wasmtime::ensure!(found.is_none(), "duplicate tina.embedder-abi section");
                found = Some(&bytes[name_end..end]);
            }
        }
        offset = end;
    }
    let payload = found.ok_or_else(|| wasmtime::Error::msg("missing tina.embedder-abi section"))?;
    wasmtime::ensure!(
        payload.len() == 4,
        "tina.embedder-abi payload must be exactly four bytes"
    );
    let version = u32::from_le_bytes(payload.try_into().unwrap());
    wasmtime::ensure!(
        version == ABI_VERSION,
        "unsupported embedder ABI version {version}"
    );
    Ok(())
}

fn read_uleb(bytes: &[u8], offset: &mut usize) -> Result<u32> {
    let mut value = 0u32;
    let mut shift = 0;
    loop {
        let byte = *bytes
            .get(*offset)
            .ok_or_else(|| wasmtime::Error::msg("truncated section length"))?;
        *offset += 1;
        wasmtime::ensure!(
            shift <= 28 && !(shift == 28 && byte > 0x0f),
            "oversized section length"
        );
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
}

fn validate_probe_component(
    engine: &Engine,
    component: &Component,
    policy: PluginPolicy,
) -> Result<()> {
    use ComponentItem as Item;
    let ty = component.component_type();
    let imports: Vec<_> = ty.imports(engine).collect();
    let mut imported = BTreeMap::new();
    for (name, item) in imports {
        validate_import_name(name, policy)?;
        wasmtime::ensure!(!imported.contains_key(name), "duplicate import {name}");
        imported.insert(name.to_owned(), item);
    }
    wasmtime::ensure!(
        imported.contains_key(SERVICE_TYPES),
        "missing service-types import"
    );
    wasmtime::ensure!(
        imported.contains_key(ENGINE_SERVICE),
        "missing engine-service import"
    );
    wasmtime::ensure!(
        policy.allow_engine_service_import,
        "engine-service interface import denied by host policy"
    );

    let service_members = interface_members(engine, imported.get(SERVICE_TYPES).unwrap())?;
    exact_names(&service_members, &["service"], SERVICE_TYPES)?;
    let service_type = match service_members.get("service").unwrap() {
        Item::Resource(r) => *r,
        _ => wasmtime::bail!("service-types.service must be a resource"),
    };
    let engine_members = interface_members(engine, imported.get(ENGINE_SERVICE).unwrap())?;
    exact_names(
        &engine_members,
        &["call", "service", "service-error"],
        ENGINE_SERVICE,
    )?;
    let aliased_service = match engine_members.get("service").unwrap() {
        Item::Resource(r) => *r,
        _ => wasmtime::bail!("engine-service.service must be a resource"),
    };
    wasmtime::ensure!(
        aliased_service == service_type,
        "engine-service resource identity differs from service-types"
    );
    let service_error = require_service_error(engine_members.get("service-error").unwrap())?;
    let call = require_func(engine_members.get("call").unwrap(), "engine-service.call")?;
    validate_call_signature(&call, service_type, service_error.clone())?;

    if let Some(item) = imported.get("wasm:dart/trace@1.0.0") {
        let members = interface_members(engine, item)?;
        exact_names(&members, &["capture-utf16"], "wasm:dart/trace")?;
        require_func(members.get("capture-utf16").unwrap(), "trace.capture-utf16")?;
    }
    if let Some(item) = imported.get("wasi:random/insecure@0.3.0") {
        let members = interface_members(engine, item)?;
        allowed_names(
            &members,
            &["get-insecure-random-bytes", "get-insecure-random-u64"],
            "wasi:random/insecure",
        )?;
        for (name, member) in &members {
            require_func(member, name)?;
        }
    }
    if let Some(item) = imported.get("wasi:cli/types@0.3.0") {
        let members = interface_members(engine, item)?;
        allowed_names(&members, &["error-code"], "wasi:cli/types")?;
        if let Some(member) = members.get("error-code") {
            wasmtime::ensure!(
                matches!(member, Item::Type(Type::Enum(_))),
                "wasi:cli/types.error-code must be an enum"
            );
        }
    }
    if let Some(item) = imported.get("wasi:cli/stdout@0.3.0") {
        let members = interface_members(engine, item)?;
        allowed_names(&members, &["write-via-stream"], "wasi:cli/stdout")?;
        if let Some(member) = members.get("write-via-stream") {
            require_func(member, "wasi:cli/stdout.write-via-stream")?;
        }
    }

    let exports: Vec<_> = ty.exports(engine).collect();
    validate_export_names(&exports.iter().map(|(name, _)| *name).collect::<Vec<_>>())?;
    let plugin_members = interface_members(engine, &exports[0].1)?;
    exact_names(
        &plugin_members,
        &["describe", "descriptor", "plugin-error", "run"],
        PLUGIN_INTERFACE,
    )?;
    validate_descriptor_type(plugin_members.get("descriptor").unwrap())?;
    validate_plugin_error(plugin_members.get("plugin-error").unwrap(), service_error)?;
    validate_run_signature(
        &require_func(plugin_members.get("run").unwrap(), "plugin.run")?,
        service_type,
        plugin_members.get("plugin-error").unwrap(),
    )?;
    validate_describe_signature(
        &require_func(plugin_members.get("describe").unwrap(), "plugin.describe")?,
        plugin_members.get("descriptor").unwrap(),
    )?;
    Ok(())
}

fn validate_import_name(name: &str, policy: PluginPolicy) -> Result<()> {
    match name {
        SERVICE_TYPES => Ok(()), // type identity only; grants no callable authority
        ENGINE_SERVICE if policy.allow_engine_service_import => Ok(()),
        ENGINE_SERVICE => wasmtime::bail!("engine-service interface import denied by host policy"),
        "wasm:dart/trace@1.0.0"
        | "wasi:random/insecure@0.3.0"
        | "wasi:cli/types@0.3.0"
        | "wasi:cli/stdout@0.3.0" => Ok(()),
        _ => wasmtime::bail!("unsupported component import {name}"),
    }
}

fn interface_members(
    engine: &Engine,
    item: &wasmtime::component::types::ComponentExtern<'_>,
) -> Result<BTreeMap<String, ComponentItem>> {
    let instance = match &item.ty {
        ComponentItem::ComponentInstance(instance) => instance,
        _ => wasmtime::bail!("component import/export must be an interface instance"),
    };
    Ok(instance
        .exports(engine)
        .map(|(name, item)| (name.to_owned(), item.ty))
        .collect())
}

fn exact_names(
    members: &BTreeMap<String, ComponentItem>,
    names: &[&str],
    where_: &str,
) -> Result<()> {
    let actual: Vec<_> = members.keys().map(String::as_str).collect();
    wasmtime::ensure!(
        actual.len() == names.len() && names.iter().all(|name| members.contains_key(*name)),
        "unsupported members in {where_}: {actual:?}"
    );
    Ok(())
}

fn allowed_names(
    members: &BTreeMap<String, ComponentItem>,
    names: &[&str],
    where_: &str,
) -> Result<()> {
    wasmtime::ensure!(
        members.keys().all(|name| names.contains(&name.as_str())),
        "unsupported member in {where_}"
    );
    Ok(())
}

fn validate_export_names(names: &[&str]) -> Result<()> {
    wasmtime::ensure!(
        names.len() == 1 && names[0] == PLUGIN_INTERFACE,
        "unsupported plugin export set or world version"
    );
    Ok(())
}

fn require_func(item: &ComponentItem, name: &str) -> Result<ComponentFunc> {
    match item {
        ComponentItem::ComponentFunc(func) => Ok(func.clone()),
        _ => wasmtime::bail!("{name} must be a function"),
    }
}

fn validate_call_signature(
    func: &ComponentFunc,
    service: ResourceType,
    service_error: Type,
) -> Result<()> {
    wasmtime::ensure!(
        func.params().count() == 2 && func.results().count() == 1,
        "wrong engine-service.call arity"
    );
    let params: Vec<_> = func.params().collect();
    wasmtime::ensure!(
        params[0].0 == "service" && params[0].1 == Type::Borrow(service),
        "wrong engine-service.call resource identity"
    );
    wasmtime::ensure!(
        params[1].0 == "request" && params[1].1 == Type::String,
        "wrong engine-service.call request type"
    );
    let results: Vec<_> = func.results().collect();
    wasmtime::ensure!(
        is_result(&results[0], Some(&Type::String), Some(&service_error)),
        "wrong engine-service.call result type"
    );
    wasmtime::ensure!(!func.async_(), "async engine-service.call is unsupported");
    Ok(())
}

fn require_service_error(item: &ComponentItem) -> Result<Type> {
    let ComponentItem::Type(Type::Variant(variant)) = item else {
        wasmtime::bail!("service-error must be a variant")
    };
    let cases: Vec<_> = variant.cases().collect();
    wasmtime::ensure!(
        cases.len() == 2
            && cases[0].name == "denied"
            && cases[0].ty.is_none()
            && cases[1].name == "unavailable"
            && cases[1].ty.is_none(),
        "unsupported service-error variant"
    );
    Ok(Type::Variant(variant.clone()))
}

fn validate_descriptor_type(item: &ComponentItem) -> Result<()> {
    let ComponentItem::Type(Type::Record(record)) = item else {
        wasmtime::bail!("descriptor must be a record")
    };
    let fields: Vec<_> = record.fields().collect();
    wasmtime::ensure!(
        fields.len() == 2
            && fields[0].name == "id"
            && fields[0].ty == Type::String
            && fields[1].name == "description"
            && fields[1].ty == Type::String,
        "unsupported descriptor record"
    );
    Ok(())
}

fn validate_plugin_error(item: &ComponentItem, expected_service_error: Type) -> Result<()> {
    let ComponentItem::Type(Type::Variant(variant)) = item else {
        wasmtime::bail!("plugin-error must be a variant")
    };
    let cases: Vec<_> = variant.cases().collect();
    wasmtime::ensure!(
        cases.len() == 3
            && cases[0].name == "rejected"
            && cases[0].ty == Some(Type::String)
            && cases[1].name == "failed"
            && cases[1].ty == Some(Type::String)
            && cases[2].name == "service",
        "unsupported plugin-error variant"
    );
    let Some(Type::Variant(service_error)) = &cases[2].ty else {
        wasmtime::bail!("plugin-error.service must be a service-error variant")
    };
    wasmtime::ensure!(
        Type::Variant(service_error.clone()) == expected_service_error,
        "plugin-error.service identity differs from imported service-error"
    );
    let service_cases: Vec<_> = service_error.cases().collect();
    wasmtime::ensure!(
        service_cases.len() == 2
            && service_cases[0].name == "denied"
            && service_cases[0].ty.is_none()
            && service_cases[1].name == "unavailable"
            && service_cases[1].ty.is_none(),
        "unsupported plugin-error.service cases"
    );
    Ok(())
}

fn validate_run_signature(
    func: &ComponentFunc,
    service: ResourceType,
    error_item: &ComponentItem,
) -> Result<()> {
    wasmtime::ensure!(
        func.params().count() == 2 && func.results().count() == 1,
        "wrong plugin.run arity"
    );
    let params: Vec<_> = func.params().collect();
    wasmtime::ensure!(
        params[0].0 == "service" && params[0].1 == Type::Borrow(service),
        "wrong plugin.run service identity"
    );
    wasmtime::ensure!(
        params[1].0 == "input" && params[1].1 == Type::String,
        "wrong plugin.run input type"
    );
    let ComponentItem::Type(error_type) = error_item else {
        wasmtime::bail!("plugin-error must be a type")
    };
    let results: Vec<_> = func.results().collect();
    wasmtime::ensure!(
        matches!(&results[0], Type::Result(result) if result.ok() == Some(Type::String) && result.err() == Some(error_type.clone())),
        "wrong plugin.run result type"
    );
    wasmtime::ensure!(
        !func.async_(),
        "async plugin.run is unsupported by probe loader"
    );
    Ok(())
}

fn validate_describe_signature(func: &ComponentFunc, descriptor: &ComponentItem) -> Result<()> {
    wasmtime::ensure!(
        func.params().count() == 0 && func.results().count() == 1,
        "wrong plugin.describe arity"
    );
    let ComponentItem::Type(expected) = descriptor else {
        wasmtime::bail!("descriptor must be a type")
    };
    let results: Vec<_> = func.results().collect();
    wasmtime::ensure!(results[0] == *expected, "wrong plugin.describe result type");
    wasmtime::ensure!(
        !func.async_(),
        "async plugin.describe is unsupported by probe loader"
    );
    Ok(())
}

fn is_result(value: &Type, ok: Option<&Type>, err: Option<&Type>) -> bool {
    matches!(value, Type::Result(result) if result.ok().as_ref() == ok && result.err().as_ref() == err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    fn custom_section(payload: &[u8]) -> Vec<u8> {
        let mut body = vec![ABI_SECTION.len() as u8];
        body.extend_from_slice(ABI_SECTION);
        body.extend_from_slice(payload);
        let mut section = vec![0, body.len() as u8];
        section.extend(body);
        section
    }

    #[test]
    fn embedder_stamp_is_exact_and_fail_closed() {
        let header = b"\0asm\r\0\x01\0";
        let mut valid = header.to_vec();
        valid.extend(custom_section(&EMBEDDER_ABI_VERSION.to_le_bytes()));
        assert!(verify_embedder_abi(&valid).is_ok());
        assert!(verify_embedder_abi(header).is_err());
        let mut duplicate = valid.clone();
        duplicate.extend(custom_section(&EMBEDDER_ABI_VERSION.to_le_bytes()));
        assert!(verify_embedder_abi(&duplicate).is_err());
        let mut malformed = header.to_vec();
        malformed.extend(custom_section(&[1, 0, 0]));
        assert!(verify_embedder_abi(&malformed).is_err());
        let mut mismatch = header.to_vec();
        mismatch.extend(custom_section(&2u32.to_le_bytes()));
        assert!(verify_embedder_abi(&mismatch).is_err());
        assert!(
            verify_embedder_abi(&[header.as_slice(), &[0, 0xff, 0xff, 0xff, 0xff, 0x10]].concat())
                .is_err()
        );
        assert!(verify_embedder_abi(&[header.as_slice(), &[0, 0x80]].concat()).is_err());
    }

    #[test]
    fn import_policy_is_exact_and_separate_from_operation_grant() {
        let allow = PluginPolicy {
            allow_engine_service_import: true,
            allow_engine_service_call: false,
        };
        assert!(validate_import_name(SERVICE_TYPES, allow).is_ok());
        assert!(validate_import_name(ENGINE_SERVICE, allow).is_ok());
        assert!(validate_import_name(ENGINE_SERVICE, PluginPolicy::default()).is_err());
        assert!(validate_import_name("tina:plugin-probe/engine-service@0.2.0", allow).is_err());
        assert!(validate_import_name("wasi:filesystem/types@0.3.0", allow).is_err());
        assert!(validate_export_names(&["tina:plugin-probe/plugin@0.2.0"]).is_err());
        assert!(validate_export_names(&[]).is_err());
    }

    #[test]
    fn abi_refusal_precedes_component_parsing() -> Result<()> {
        let mut malformed = b"\0asm\r\0\x01\0".to_vec();
        malformed.extend(custom_section(&2u32.to_le_bytes()));
        let mut boundary = PluginBoundary::new(engine()?, Arc::new(CounterBroker::default()))?;
        let error = boundary.load(&malformed, allow_call()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported embedder ABI version 2"),
            "{error}"
        );
        assert!(boundary.plugins.is_empty());
        Ok(())
    }

    #[derive(Default)]
    struct CounterBroker {
        value: Mutex<i32>,
        calls: AtomicUsize,
        callers: Mutex<Vec<PluginId>>,
    }

    impl PluginServiceBroker for CounterBroker {
        fn call(
            &self,
            caller: ServiceCaller,
            request: &str,
            _frame: Option<&PluginCallFrame>,
        ) -> std::result::Result<String, ServiceCallError> {
            assert!(caller.call_number() > 0);
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.callers.lock().unwrap().push(caller.plugin());
            let (operation, run) = request.split_once(";run=").unwrap_or((request, ""));
            let Some(delta) = operation
                .strip_prefix("add:")
                .and_then(|s| s.parse::<i32>().ok())
            else {
                return Err(ServiceCallError::Denied);
            };
            let mut value = self.value.lock().unwrap();
            *value += delta;
            Ok(format!("counter={};plugin-run={run}", *value))
        }
    }

    fn engine() -> Result<Engine> {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        Engine::new(&config)
    }

    fn allow_call() -> PluginPolicy {
        PluginPolicy {
            allow_engine_service_import: true,
            allow_engine_service_call: true,
        }
    }

    fn expect_actual_component_schema_rejection(bytes: &[u8], expected: &str) -> Result<()> {
        verify_embedder_abi(bytes)?;
        let engine = engine()?;
        let component =
            Component::new(&engine, bytes).context("negative fixture must be a valid component")?;
        let error = validate_probe_component(&engine, &component, allow_call()).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "expected {expected:?}, got {error}"
        );
        Ok(())
    }

    #[test]
    fn actual_dart_component_admission_rejects_versions_members_and_schemas() -> Result<()> {
        expect_actual_component_schema_rejection(
            include_bytes!(
                "../testdata/plugin_probe/admission-fixtures/wrong-engine-service-version.component.wasm"
            ),
            "unsupported component import tina:plugin-probe/engine-service@0.2.0",
        )?;
        expect_actual_component_schema_rejection(
            include_bytes!(
                "../testdata/plugin_probe/admission-fixtures/wrong-engine-service-member.component.wasm"
            ),
            "unsupported members in tina:plugin-probe/engine-service",
        )?;
        expect_actual_component_schema_rejection(
            include_bytes!(
                "../testdata/plugin_probe/admission-fixtures/wrong-plugin-export-version.component.wasm"
            ),
            "unsupported plugin export set or world version",
        )?;
        expect_actual_component_schema_rejection(
            include_bytes!(
                "../testdata/plugin_probe/admission-fixtures/wrong-run-param.component.wasm"
            ),
            "wrong plugin.run input type",
        )?;
        expect_actual_component_schema_rejection(
            include_bytes!(
                "../testdata/plugin_probe/admission-fixtures/wrong-descriptor-field.component.wasm"
            ),
            "unsupported descriptor record",
        )?;
        expect_actual_component_schema_rejection(
            include_bytes!(
                "../testdata/plugin_probe/admission-fixtures/wrong-run-resource-ownership.component.wasm"
            ),
            "wrong plugin.run service identity",
        )?;
        Ok(())
    }

    #[test]
    fn import_admission_precedes_guest_initialization() -> Result<()> {
        let bytes =
            include_bytes!("../testdata/plugin_probe/admission-fixtures/init-trap.component.wasm");
        let broker = Arc::new(CounterBroker::default());
        let mut boundary = PluginBoundary::new(engine()?, broker.clone())?;
        let denied = boundary.load(bytes, PluginPolicy::default()).unwrap_err();
        assert!(
            denied
                .to_string()
                .contains("engine-service interface import denied by host policy"),
            "{denied}"
        );
        assert!(boundary.plugins.is_empty());
        assert_eq!(boundary.next_slot, 1);
        assert_eq!(boundary.next_generation, 1);
        assert_eq!(broker.calls.load(Ordering::SeqCst), 0);
        let allowed = boundary.load(bytes, allow_call()).unwrap_err();
        let detail = format!("{allowed:#}");
        assert!(
            detail.contains("instantiating plugin component"),
            "allowed fixture should reach trapping core start: {detail}"
        );
        assert!(
            detail.to_lowercase().contains("unreachable"),
            "allowed fixture must fail specifically in its unreachable start: {detail}"
        );
        assert!(
            !detail.contains("unsupported component import"),
            "admission should pass before start trap: {detail}"
        );
        assert!(boundary.plugins.is_empty());
        assert_eq!(boundary.next_slot, 1);
        assert_eq!(boundary.next_generation, 1);
        assert_eq!(broker.calls.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn real_dart_component_admission_capability_slots_and_reload() -> Result<()> {
        let bytes = include_bytes!("../testdata/plugin_probe/plugin.component.wasm");
        let broker = Arc::new(CounterBroker {
            value: Mutex::new(100),
            calls: AtomicUsize::new(0),
            callers: Mutex::new(Vec::new()),
        });
        let mut boundary = PluginBoundary::new(engine()?, broker.clone())?;

        // Interface admission is checked before Store creation/instantiation.
        assert!(boundary.load(bytes, PluginPolicy::default()).is_err());
        assert_eq!(broker.calls.load(Ordering::SeqCst), 0);
        assert!(boundary.plugins.is_empty());

        let first = boundary.load(bytes, allow_call())?;
        let second = boundary.load(bytes, allow_call())?;
        let denied_call = boundary.load(
            bytes,
            PluginPolicy {
                allow_engine_service_import: true,
                allow_engine_service_call: false,
            },
        )?;
        assert_ne!(first.slot, second.slot);
        assert_ne!(second.slot, denied_call.slot);
        let desc = boundary.describe(first)?;
        assert_eq!(desc.id, "example/stateful-echo");
        assert_eq!(
            boundary.run(first, "add:5")?,
            Ok("counter=105;plugin-run=1".into())
        );
        assert_eq!(
            boundary.run(first, "add:0")?,
            Ok("counter=105;plugin-run=2".into())
        );
        assert_eq!(
            boundary.run(second, "add:0")?,
            Ok("counter=105;plugin-run=1".into())
        );
        let callers = broker.callers.lock().unwrap().clone();
        assert_eq!(
            callers,
            vec![first, first, second],
            "broker must see host-issued per-plugin identity"
        );

        let calls_before_denied = broker.calls.load(Ordering::SeqCst);
        assert_eq!(
            boundary.run(denied_call, "add:20")?,
            Err(PluginFailure::ServiceDenied)
        );
        assert_eq!(
            broker.calls.load(Ordering::SeqCst),
            calls_before_denied,
            "operation grant refusal must not reach broker"
        );

        // A separate request is denied by the trusted broker and does not
        // mutate its state even when the generic call interface is granted.
        assert_eq!(
            boundary.run(first, "secret:99")?,
            Err(PluginFailure::ServiceDenied)
        );
        assert_eq!(*broker.value.lock().unwrap(), 105);

        // Loader identity and globally unique generations reject cross-loader
        // and cross-slot forged combinations before invoking Wasmtime.
        let mut other = PluginBoundary::new(engine()?, broker.clone())?;
        let other_id = other.load(bytes, allow_call())?;
        assert!(other.describe(first).is_err());
        let mixed = PluginId {
            loader: first.loader,
            slot: second.slot,
            generation: first.generation,
        };
        let calls_before_stale = broker.calls.load(Ordering::SeqCst);
        assert!(boundary.run(mixed, "add:999").is_err());
        assert_eq!(broker.calls.load(Ordering::SeqCst), calls_before_stale);

        boundary.retire(first)?;
        assert!(boundary.describe(first).is_err());
        assert_eq!(boundary.describe(second)?.id, "example/stateful-echo");
        assert_eq!(
            boundary.run(second, "add:0")?,
            Ok("counter=105;plugin-run=2".into())
        );
        assert_eq!(
            boundary.run(denied_call, "add:1")?,
            Err(PluginFailure::ServiceDenied)
        );
        boundary.retire(denied_call)?;
        boundary.retire(second)?;
        let reloaded = boundary.load(bytes, allow_call())?;
        assert_eq!(reloaded.slot, second.slot, "retired slots should be reused");
        assert!(reloaded.generation > denied_call.generation);
        assert!(boundary.run(first, "add:1000").is_err());
        assert!(boundary.run(second, "add:1000").is_err());
        assert_eq!(
            boundary.run(reloaded, "add:1")?,
            Ok("counter=106;plugin-run=1".into())
        );
        boundary.retire(reloaded)?;
        other.retire(other_id)?;
        assert_eq!(*broker.value.lock().unwrap(), 106);
        Ok(())
    }

    #[test]
    fn generation_exhaustion_fails_before_slot_reservation() -> Result<()> {
        let bytes = include_bytes!("../testdata/plugin_probe/plugin.component.wasm");
        let broker = Arc::new(CounterBroker::default());
        let mut boundary = PluginBoundary::new(engine()?, broker)?;
        boundary.next_generation = u64::MAX;
        assert!(boundary.load(bytes, allow_call()).is_err());
        assert_eq!(boundary.next_slot, 1);
        assert!(boundary.plugins.is_empty());
        Ok(())
    }

    #[test]
    fn retirement_cleanup_failure_still_revokes_and_recycles_slot() -> Result<()> {
        let bytes = include_bytes!("../testdata/plugin_probe/plugin.component.wasm");
        let broker = Arc::new(CounterBroker {
            value: Mutex::new(10),
            calls: AtomicUsize::new(0),
            callers: Mutex::new(Vec::new()),
        });
        let mut boundary = PluginBoundary::new(engine()?, broker.clone())?;
        let broken = boundary.load(bytes, allow_call())?;
        let survivor = boundary.load(bytes, allow_call())?;
        let proxy_rep = boundary.plugins.get(&broken.slot).unwrap().proxy_rep;
        // Inject a table-cleanup failure after Wasmtime's dynamic resource is
        // revoked; this models a partially corrupted Store during retirement.
        let _removed = boundary
            .plugins
            .get_mut(&broken.slot)
            .unwrap()
            .store
            .data_mut()
            .table
            .delete(Resource::<Proxy>::new_own(proxy_rep))?;
        assert!(boundary.retire(broken).is_err());
        assert!(boundary.run(broken, "add:100").is_err());
        assert_eq!(
            boundary.run(survivor, "add:0")?,
            Ok("counter=10;plugin-run=1".into())
        );
        let replacement = boundary.load(bytes, allow_call())?;
        assert_eq!(replacement.slot, broken.slot);
        assert!(replacement.generation > broken.generation);
        assert!(boundary.run(broken, "add:1000").is_err());
        assert_eq!(
            boundary.run(replacement, "add:1")?,
            Ok("counter=11;plugin-run=1".into())
        );
        boundary.retire(survivor)?;
        boundary.retire(replacement)?;
        assert_eq!(broker.calls.load(Ordering::SeqCst), 2);
        Ok(())
    }
}
