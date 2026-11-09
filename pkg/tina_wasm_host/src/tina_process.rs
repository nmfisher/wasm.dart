//! The `tina:process` host import: the engine's process port served by
//! the host, because WASI has no process spawn. Spec: `docs/tina_process.md`.
//!
//! The engine sends the access profile with every request; this side is
//! the only side that can spawn, so it is the side that enforces: the
//! profile is evaluated before any process exists, an `ask` verdict
//! returns to the engine as an approval request (the host cannot call
//! back into a blocked guest), and the child's stdout/stderr stream to
//! the guest through the same `StreamReader` machinery `tina:http`
//! bodies use - the guest-side waitable-set join serves them unchanged.
//!
//! The interface (see `wit/tina_process.wit`) is deliberately minimal:
//! `spawn`, `respond`, `wait`, `kill`. No shell, no pseudoterminal, no
//! globbing: `argv` is a list of literal strings.

use std::{
    collections::HashMap,
    io::Write as _,
    path::PathBuf,
    pin::Pin,
    task::{Context, Poll},
};

use bytes::{Buf, Bytes, BytesMut};
use tokio::{
    io::{AsyncRead, AsyncWriteExt as _, ReadBuf},
    process::{Child, ChildStderr, ChildStdout, Command},
};
use std::process::Stdio;
use wasmtime::component::{
    Accessor, Destination, StreamReader, StreamProducer, StreamResult,
};
use wasmtime::StoreContextMut;
use wasmtime::AsContextMut as _;

pub mod bindings {
    wasmtime::component::bindgen!({
        path: "wit/deps/tina-process",
        world: "tina:process/tina-process-host",
        imports: {
            // Like `tina:http`: the host spawns real processes and hands
            // stream handles to the guest, which needs the concurrent
            // component-model runtime, so the import is lowered as async
            // with store access (the children and pending approvals live
            // in the store state).
            "tina:process/process": async | store,
        },
    });
}

use bindings::tina::process::types::{
    AccessProfile, ApprovalRequest, Error, Exit, Outcome, Rule, SpawnRequest,
    Started, Verdict,
};

// The store state doubles as the host implementation; see `tina_http.rs`
// for why `HasData` is implemented on the concrete state (only once per
// state type - `tina_http` owns the impl).

impl bindings::tina::process::process::Host for crate::HostState {}

/// Adds the `tina:process/process@1.0.0` instance to `linker`.
/// Components that do not import `tina:process` never reach it.
pub fn add_to_linker(
    linker: &mut wasmtime::component::Linker<crate::HostState>,
) -> wasmtime::Result<()> {
    bindings::tina::process::process::add_to_linker::<crate::HostState, crate::HostState>(
        linker,
        |state| state,
    )
}

/// The `tina:process` part of the store state: live children (id ->
/// child, `kill_on_drop` so an abandoned child cannot outlive the store)
/// and spawn requests waiting on an approval answer.
#[derive(Default)]
pub struct ProcessHost {
    next_id: u64,
    children: HashMap<u64, Child>,
    approvals: HashMap<u64, PendingSpawn>,
}

/// A spawn the profile parked behind an approval round-trip: the child
/// does not exist until the engine answers `respond`.
struct PendingSpawn {
    argv: Vec<String>,
    cwd: PathBuf,
    env: Vec<(String, String)>,
    stdin: Option<Vec<u8>>,
}

/// The profile verdict for one command line.
enum Decision {
    Allow,
    Denied { reason: String },
    Ask { reason: String },
}

/// Evaluates the access profile against the joined command line:
/// first anchored match decides, no match asks, an invalid pattern
/// fails closed. The reason strings are interface contract (the native
/// port produces the same ones); see the spec.
fn evaluate(profile: &AccessProfile, command: &str) -> Decision {
    evaluate_rules(&profile.rules, command)
}

fn evaluate_rules(rules: &[Rule], command: &str) -> Decision {
    for (i, rule) in rules.iter().enumerate() {
        // Anchored like tina_mode's command patterns: the pattern must
        // cover the whole line, so a `git status` rule never answers
        // `git status --porcelain`.
        let anchored = format!("^(?:{})$", rule.pattern);
        let compiled = match regex::Regex::new(&anchored) {
            Ok(compiled) => compiled,
            Err(_) => {
                return Decision::Denied {
                    reason: format!("rule {i} ({}) is not a valid pattern", rule.pattern),
                }
            }
        };
        if !compiled.is_match(command) {
            continue;
        }
        return match rule.verdict {
            Verdict::Allow => Decision::Allow,
            Verdict::Deny => Decision::Denied {
                reason: format!("rule {i} ({}) denies the command", rule.pattern),
            },
            Verdict::Ask => Decision::Ask {
                reason: format!("rule {i} ({}) requires approval", rule.pattern),
            },
        };
    }
    // No rule matched: route to the human, not to silent refusal.
    Decision::Ask {
        reason: "no rule matched the command".to_string(),
    }
}

/// Resolves a guest `cwd` against the runner's preopens: `..` is
/// refused, a relative path belongs to the first `.`/empty preopen (or a
/// matching prefix), and anything no preopen covers is an error - the
/// same confinement `WasmFileSystemPort` applies. `None` (the engine
/// passed no working directory) is the first preopen: an engine under
/// this host has no other working directory.
fn resolve_cwd(
    preopens: &[(String, PathBuf)],
    guest: Option<&str>,
) -> Result<PathBuf, String> {
    let Some(path) = guest else {
        return preopens
            .first()
            .map(|(_, host)| host.clone())
            .ok_or_else(|| "no preopened directory to spawn in".to_string());
    };
    let mut candidate = path;
    while let Some(rest) = candidate.strip_prefix("./") {
        candidate = rest;
    }
    if candidate.split('/').any(|segment| segment == "..") {
        return Err(format!("cwd {path} climbs out of a preopen with `..`"));
    }
    // Longest guest path first, so nested preopens win over their parent
    // (the order `WasmPreopens` sorts to).
    let mut order: Vec<&(String, PathBuf)> = preopens.iter().collect();
    order.sort_by_key(|(guest, _)| std::cmp::Reverse(guest.len()));
    if candidate.starts_with('/') {
        for (guest_path, host) in order {
            if candidate == guest_path {
                return Ok(host.clone());
            }
            let root = prefix(guest_path);
            if candidate.starts_with(&root) {
                return Ok(host.join(&candidate[root.len()..]));
            }
        }
        return Err(format!("cwd {path} is outside the preopened directories"));
    }
    for (guest_path, host) in order {
        if guest_path == "." || guest_path.is_empty() {
            return Ok(host.join(candidate));
        }
        if candidate == guest_path {
            return Ok(host.clone());
        }
        let root = prefix(guest_path);
        if candidate.starts_with(&root) {
            return Ok(host.join(&candidate[root.len()..]));
        }
    }
    Err(format!("cwd {path} is outside the preopened directories"))
}

fn prefix(guest_path: &str) -> String {
    if guest_path.ends_with('/') {
        guest_path.to_string()
    } else {
        format!("{guest_path}/")
    }
}

/// Launches the child of a `PendingSpawn` and registers it: pipes both
/// outputs, writes `stdin` from a task once the guest yields, and leaves
/// the `Child` (kill-on-drop) under its id. The pipes are returned for
/// the caller to expose as guest streams.
fn launch_child(
    state: &mut crate::HostState,
    pending: PendingSpawn,
) -> Result<(u64, ChildStdout, ChildStderr), String> {
    let mut argv = pending.argv.iter();
    let Some(program) = argv.next() else {
        return Err("argv must not be empty".to_string());
    };
    if program.is_empty() {
        return Err("argv[0] must not be empty".to_string());
    }
    let mut command = Command::new(program);
    command
        .args(argv)
        .current_dir(&pending.cwd)
        // An engine that never waits or kills still cannot leak a child:
        // dropping the store drops the Child, which kills it.
        .kill_on_drop(true)
        .stdin(if pending.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in &pending.env {
        command.env(name, value);
    }
    let mut child = command
        .spawn()
        .map_err(|err| format!("could not run {program}: {err}"))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    if let Some(bytes) = pending.stdin {
        if let Some(mut stdin) = child.stdin.take() {
            // The write happens in its own task: a child that ignores a
            // large stdin must not block the spawn call that started it.
            tokio::spawn(async move {
                let _ = stdin.write_all(&bytes).await;
                let _ = stdin.shutdown().await;
            });
        }
    }
    let (stdout, stderr) = match (stdout, stderr) {
        (Some(stdout), Some(stderr)) => (stdout, stderr),
        _ => return Err(format!("could not pipe the output of {program}")),
    };

    state.process.next_id += 1;
    let id = state.process.next_id;
    state.process.children.insert(id, child);
    Ok((id, stdout, stderr))
}

fn unknown_process(id: u64) -> Error {
    Error::Message(format!("unknown process id {id}"))
}

/// Takes a child out of the table for `wait` (which reaps it); a second
/// wait on the same id then reports the id as unknown.
fn take_child(state: &mut crate::HostState, id: u64) -> Result<Child, Error> {
    state
        .process
        .children
        .remove(&id)
        .ok_or_else(|| unknown_process(id))
}

/// Waits a taken child and maps its status to the interface's `exit`:
/// the raw exit code, or the signal that terminated it (the Dart side
/// maps `signal(n)` to `-n`, matching `dart:io`).
async fn wait_child(mut child: Child, id: u64) -> Result<Exit, Error> {
    let status = child
        .wait()
        .await
        .map_err(|err| Error::Message(format!("waiting for process {id}: {err}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return Ok(Exit::Signal(signal as u32));
        }
    }
    let code = status
        .code()
        .ok_or_else(|| Error::Message(format!("process {id} reported no exit status")))?;
    Ok(Exit::Code(u32::try_from(code).unwrap_or(0)))
}

/// One pipe (child stdout or stderr) as a `StreamProducer`: bytes flow
/// on demand into the guest's read, chunked as the pipe delivers them,
/// and end-of-pipe ends the stream cleanly, like a delivered HTTP body.
struct PipeProducer<R: AsyncRead + Unpin> {
    reader: R,
    scratch: Vec<u8>,
    carry: Option<Bytes>,
}

impl<R: AsyncRead + Unpin> PipeProducer<R> {
    fn new(reader: R) -> Self {
        PipeProducer {
            reader,
            scratch: vec![0; 64 * 1024],
            carry: None,
        }
    }
}

impl<D, R: AsyncRead + Unpin + Send + 'static> StreamProducer<D> for PipeProducer<R> {
    type Item = u8;
    type Buffer = BytesMut;

    fn poll_produce<'a>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        // An empty destination is the guest asking to be woken when the
        // stream is readable; report completion and let the next read
        // retry (same caveat as the HTTP body producer).
        if dst.remaining(store.as_context_mut()) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        let mut dst = dst.as_direct(store, 64 * 1024);
        let this = self.get_mut();

        loop {
            // Leftover bytes from a previous read go out first.
            if let Some(mut chunk) = this.carry.take() {
                let n = dst
                    .write(&chunk)
                    .expect("DirectDestination::write cannot fail");
                chunk.advance(n);
                if !chunk.is_empty() {
                    this.carry = Some(chunk);
                }
                return Poll::Ready(Ok(StreamResult::Completed));
            }

            let mut read = ReadBuf::new(&mut this.scratch);
            match Pin::new(&mut this.reader).poll_read(cx, &mut read) {
                Poll::Ready(Ok(())) if read.filled().is_empty() => {
                    // The child closed its end: the stream ends cleanly.
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
                Poll::Ready(Ok(())) => {
                    let filled = read.filled();
                    let n = dst
                        .write(filled)
                        .expect("DirectDestination::write cannot fail");
                    if n < filled.len() {
                        this.carry = Some(Bytes::copy_from_slice(&filled[n..]));
                    }
                    return Poll::Ready(Ok(StreamResult::Completed));
                }
                // A pipe read errors when the child died without closing
                // cleanly; the output delivered so far stands.
                Poll::Ready(Err(_)) => return Poll::Ready(Ok(StreamResult::Dropped)),
                Poll::Pending if finish => {
                    return Poll::Ready(Ok(StreamResult::Cancelled))
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<T> bindings::tina::process::process::HostWithStore<T> for crate::HostState
where
    T: 'static + Send,
{
    async fn spawn(
        store: &Accessor<T, crate::HostState>,
        req: SpawnRequest,
    ) -> Result<Outcome, Error> {
        if req.argv.is_empty() {
            return Err(Error::Message("argv must not be empty".to_string()));
        }
        let command_line = req.argv.join(" ");
        match evaluate(&req.profile, &command_line) {
            Decision::Allow => {
                let pending = store.with(|mut access| {
                    let state = access.get();
                    let cwd = resolve_cwd(&state.preopens, req.cwd.as_deref())
                        .map_err(Error::Message)?;
                    Ok(PendingSpawn {
                        argv: req.argv,
                        cwd,
                        env: req.env,
                        stdin: req.stdin,
                    })
                })?;
                started(store, pending).await
            }
            Decision::Denied { reason } => Ok(Outcome::Denied(reason)),
            Decision::Ask { reason } => {
                let id = store.with(|mut access| {
                    let state = access.get();
                    let cwd = resolve_cwd(&state.preopens, req.cwd.as_deref())
                        .map_err(Error::Message)?;
                    state.process.next_id += 1;
                    let id = state.process.next_id;
                    state.process.approvals.insert(
                        id,
                        PendingSpawn {
                            argv: req.argv,
                            cwd,
                            env: req.env,
                            stdin: req.stdin,
                        },
                    );
                    Ok(id)
                })?;
                Ok(Outcome::Approval(ApprovalRequest {
                    id,
                    command: command_line,
                    reason,
                }))
            }
        }
    }

    async fn respond(
        store: &Accessor<T, crate::HostState>,
        approval: u64,
        allow: bool,
    ) -> Result<Outcome, Error> {
        let pending = store.with(|mut access| {
            access.get().process.approvals.remove(&approval)
        });
        match pending {
            Some(pending) if allow => started(store, pending).await,
            Some(_) => Ok(Outcome::Denied(
                "the approval flow denied the command".to_string(),
            )),
            None => Err(Error::Message(format!(
                "unknown approval id {approval}"
            ))),
        }
    }

    async fn wait(
        store: &Accessor<T, crate::HostState>,
        id: u64,
    ) -> Result<Exit, Error> {
        let child = store.with(|mut access| take_child(access.get(), id))?;
        wait_child(child, id).await
    }

    async fn kill(
        store: &Accessor<T, crate::HostState>,
        id: u64,
    ) -> Result<(), Error> {
        store.with(|mut access| {
            let state = access.get();
            match state.process.children.get_mut(&id) {
                // `start_kill` is the non-awaiting kill: SIGKILL now, the
                // reap happens in `wait`. Idempotent by construction.
                Some(child) => child
                    .start_kill()
                    .map_err(|err| Error::Message(format!("killing process {id}: {err}"))),
                None => Err(unknown_process(id)),
            }
        })
    }
}

/// Launches an approved spawn and exposes it to the guest: the id plus
/// both pipes as `StreamReader`s - the handles the Dart side lowers with
/// the same stream machinery it uses for `tina:http` bodies.
async fn started<T>(
    store: &Accessor<T, crate::HostState>,
    pending: PendingSpawn,
) -> Result<Outcome, Error>
where
    T: 'static + Send,
{
    let (id, stdout, stderr) = store
        .with(|mut access| launch_child(access.get(), pending).map_err(Error::Message))?;
    let (stdout, stderr) = store.with(|mut access| -> Result<
        (StreamReader<u8>, StreamReader<u8>),
        Error,
    > {
        let stdout = StreamReader::new(&mut access, PipeProducer::new(stdout))
            .map_err(failed_to_expose)?;
        let stderr = StreamReader::new(&mut access, PipeProducer::new(stderr))
            .map_err(failed_to_expose)?;
        Ok((stdout, stderr))
    })?;
    Ok(Outcome::Started(Started {
        id,
        stdout,
        stderr,
    }))
}

fn failed_to_expose(err: wasmtime::Error) -> Error {
    Error::Message(format!("could not expose process output: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(pattern: &str, verdict: Verdict) -> Rule {
        Rule {
            pattern: pattern.to_string(),
            verdict,
        }
    }

    fn ask_reason(rules: &[Rule], command: &str) -> String {
        match evaluate_rules(rules, command) {
            Decision::Ask { reason } => reason,
            _ => panic!("expected Ask for {command}"),
        }
    }

    fn denied_reason(rules: &[Rule], command: &str) -> String {
        match evaluate_rules(rules, command) {
            Decision::Denied { reason } => reason,
            _ => panic!("expected Denied for {command}"),
        }
    }

    #[test]
    fn first_anchored_match_decides() {
        let rules = vec![
            rule("git status", Verdict::Allow),
            rule("rm .*", Verdict::Deny),
        ];
        assert!(matches!(
            evaluate_rules(&rules, "git status"),
            Decision::Allow
        ));
        // Anchoring: the allow rule does not cover extra arguments, and
        // a later deny rule does not override an earlier non-match.
        assert!(matches!(
            evaluate_rules(&rules, "git status --porcelain"),
            Decision::Ask { .. }
        ));
        assert!(matches!(
            evaluate_rules(&rules, "git rm -rf /"),
            Decision::Ask { .. }
        ));
        assert!(matches!(
            evaluate_rules(&rules, "rm -rf /tmp/probe-deny"),
            Decision::Denied { .. }
        ));
    }

    #[test]
    fn no_match_asks() {
        assert_eq!(ask_reason(&[], "printf hi"), "no rule matched the command");
    }

    #[test]
    fn invalid_patterns_fail_closed() {
        assert_eq!(
            denied_reason(&[rule("[unclosed", Verdict::Allow)], "anything"),
            "rule 0 ([unclosed) is not a valid pattern"
        );
    }

    #[test]
    fn denial_and_ask_reasons_are_the_contract() {
        assert_eq!(
            denied_reason(&[rule("rm .*", Verdict::Deny)], "rm x"),
            "rule 0 (rm .*) denies the command"
        );
        assert_eq!(
            ask_reason(&[rule("cargo .*", Verdict::Ask)], "cargo build"),
            "rule 0 (cargo .*) requires approval"
        );
    }

    #[test]
    fn cwd_resolves_against_preopens_like_the_file_port() {
        let preopens = vec![
            (".".to_string(), PathBuf::from("/tmp/ws")),
            ("config".to_string(), PathBuf::from("/etc/config")),
        ];
        // Absent cwd: the first preopen is the engine's workspace.
        assert_eq!(
            resolve_cwd(&preopens, None).unwrap(),
            PathBuf::from("/tmp/ws")
        );
        // Relative paths resolve against the '.' preopen.
        assert_eq!(
            resolve_cwd(&preopens, Some(".")).unwrap(),
            PathBuf::from("/tmp/ws")
        );
        assert_eq!(
            resolve_cwd(&preopens, Some("./")).unwrap(),
            PathBuf::from("/tmp/ws")
        );
        // Named preopens resolve; absolute guest paths resolve against
        // absolutely named preopens (the `--preopen=/root=...` case).
        assert_eq!(
            resolve_cwd(&preopens, Some("config")).unwrap(),
            PathBuf::from("/etc/config")
        );
        let absolute = vec![(
            "/root".to_string(),
            PathBuf::from("/tmp/ws"),
        )];
        assert_eq!(
            resolve_cwd(&absolute, Some("/root/nested")).unwrap(),
            PathBuf::from("/tmp/ws/nested")
        );
        // Confinement: `..` and foreign paths are refused.
        assert!(resolve_cwd(&preopens, Some("../out")).is_err());
        assert!(resolve_cwd(&preopens, Some("/etc/passwd")).is_err());
        assert!(resolve_cwd(&[], None).is_err());
    }

    fn host_state() -> crate::HostState {
        crate::HostState::for_tests()
    }

    fn pending(argv: &[&str], cwd: &str) -> PendingSpawn {
        PendingSpawn {
            argv: argv.iter().map(|a| a.to_string()).collect(),
            cwd: PathBuf::from(cwd),
            env: vec![("TINA_PROCESS_TEST".to_string(), "1".to_string())],
            stdin: None,
        }
    }

    /// A child that exits on its own reports its code through `wait`,
    /// using the same helper the host import's `wait` calls.
    #[test]
    fn spawn_wait_reaps_the_exit_code() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut state = host_state();
            let (id, _stdout, _stderr) =
                launch_child(&mut state, pending(&["true", "unused"], "/tmp")).unwrap();
            let exit = wait_child(take_child(&mut state, id).unwrap(), id)
                .await
                .unwrap();
            assert!(matches!(exit, Exit::Code(0)));
            // A second wait no longer knows the id.
            assert!(take_child(&mut state, id).is_err());
        });
    }

    /// `kill` terminates a running child; `wait` reports the signal, the
    /// way `dart:io` surfaces it (as a negative exit code on the Dart
    /// side of the boundary).
    #[test]
    fn kill_terminates_and_wait_reports_the_signal() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut state = host_state();
            let (id, _stdout, _stderr) =
                launch_child(&mut state, pending(&["sleep", "30"], "/tmp")).unwrap();
            state
                .process
                .children
                .get_mut(&id)
                .unwrap()
                .start_kill()
                .unwrap();
            let exit = wait_child(take_child(&mut state, id).unwrap(), id)
                .await
                .unwrap();
            #[cfg(unix)]
            assert!(matches!(exit, Exit::Signal(9)));
            #[cfg(not(unix))]
            assert!(matches!(exit, Exit::Code(c) if c != 0));
        });
    }

    /// A denied spawn never creates a process, and an unknown id never
    /// silently succeeds.
    #[test]
    fn unknown_ids_fail() {
        let mut state = host_state();
        assert!(take_child(&mut state, 99).is_err());
    }
}
