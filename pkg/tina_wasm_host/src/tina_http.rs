//! The `tina:http` host import: the engine's HTTP port served by the host
//! with `reqwest`, following the custom-import pattern the runner already
//! uses for `wasm:dart/trace` and `wasm:dart/timeline`.
//!
//! The host enforces the capability document it was launched with
//! (`docs/capability_merger.md`): the network domain entries are the
//! allowlist. A request to an allowed domain completes with the response
//! head plus the body as a component-model byte stream and a delivery
//! future - the exact shape of `wasi:cli/stdin.read-via-stream`, lowered
//! through the same `StreamReader`/`FutureReader` machinery wasmtime-wasi
//! uses for stdio. An unknown domain parks the request behind an approval
//! round-trip the engine answers with `respond` - the same shape
//! `tina:process` uses, because the host cannot call back into a blocked
//! guest. Legacy launches (no capability document) keep the unrestricted
//! pre-merger behavior.

use std::{
    collections::HashMap,
    io::Write as _,
    pin::Pin,
    task::{Context, Poll},
};

use bytes::{Buf, Bytes, BytesMut};
use futures::Stream;
use tokio::sync::oneshot;
use wasmtime::component::{
    Accessor, Destination, FutureReader, StreamReader, StreamProducer, StreamResult,
};
use wasmtime::StoreContextMut;
use wasmtime::AsContextMut as _;

pub mod bindings {
    wasmtime::component::bindgen!({
        path: "wit/deps/tina-http",
        world: "tina:http/tina-http-host",
        imports: {
            // The host does real network I/O and hands stream/future
            // handles to the guest: both need the concurrent component
            // model runtime, so the import is lowered as async with store
            // access (to reach the reqwest client in the store state).
            "tina:http/client": async | store,
        },
    });
}

use bindings::tina::http::types::{ApprovalRequest, Error, Outcome, Request, Sent};

/// The parked domain approvals: requests waiting for the engine's
/// `respond`. Nothing is on the wire until then.
#[derive(Default)]
pub struct HttpApprovals {
    next_id: u64,
    pending: HashMap<u64, Request>,
}

/// The domain verdict for one request: send it, or park it behind an
/// approval round-trip.
enum DomainVerdict {
    Proceed,
    Ask { reason: String },
}

/// Pure so tests can exercise it without a store: the capability
/// document's network entries are the allowlist; legacy mode (no
/// document) allows everything, exactly as before the merger.
fn domain_verdict(state_policy: &crate::policy::CapabilityPolicy, url: &reqwest::Url) -> DomainVerdict {
    if state_policy.source == crate::policy::PolicySource::Legacy {
        return DomainVerdict::Proceed;
    }
    let scheme = url.scheme();
    let host = url.host_str().unwrap_or_default().to_string();
    let port = url.port_or_known_default().unwrap_or(0);
    if state_policy.domain_allowed(scheme, &host, port) {
        return DomainVerdict::Proceed;
    }
    DomainVerdict::Ask {
        reason: format!("domain {host} is not in the capability allowlist"),
    }
}

impl HttpApprovals {
    fn park(&mut self, request: Request) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.pending.insert(id, request);
        id
    }

    fn take(&mut self, id: u64) -> Option<Request> {
        self.pending.remove(&id)
    }
}

// The store state doubles as the host implementation: `HasData` exposes it
// as its own view (like `HasSelf`, but on the concrete state so the client
// trait's `HostWithStore`/`Host` bounds are all satisfied locally), and the
// empty `Host` marker is what the generated linker machinery wraps in
// `&mut` when it calls the interface's functions.
impl wasmtime::component::HasData for crate::HostState {
    type Data<'a> = &'a mut crate::HostState;
}

impl bindings::tina::http::client::Host for crate::HostState {}

/// Adds the `tina:http/client@1.1.0` instance to `linker`. Components that
/// do not import `tina:http` never reach it.
pub fn add_to_linker(
    linker: &mut wasmtime::component::Linker<crate::HostState>,
) -> wasmtime::Result<()> {
    bindings::tina::http::client::add_to_linker::<crate::HostState, crate::HostState>(
        linker,
        |state| state,
    )
}
fn failed_to_expose(err: wasmtime::Error) -> Error {
    Error::Message(format!("could not expose the response body: {err}"))
}

/// One chunk-producing HTTP body: a `reqwest` byte stream plus whatever
/// bytes a previous produce call could not fit into the guest's buffer.
struct HttpBodyProducer {
    stream: Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send + Sync>>,
    carry: Option<Bytes>,
    result: Option<oneshot::Sender<String>>,
}

impl<D> StreamProducer<D> for HttpBodyProducer {
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
        // stream is readable; `StreamProducer` cannot express that wait, so
        // report completion and let the next read retry (same caveat as
        // wasmtime-wasi's stdin producer, component-model#561).
        if dst.remaining(store.as_context_mut()) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        
        // `DirectDestination` implements `io::Write` unbuffered: each
        // `write` moves as many bytes as the guest's read can take and
        // marks them written, so an oversized chunk is split instead of
        // lost - and a small chunk reaches the guest immediately, which
        // SSE consumers depend on.
        let mut dst = dst.as_direct(store, 64 * 1024);
        let this = self.get_mut();

        loop {
            // Leftover bytes from a previous chunk go out first.
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

            match this.stream.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) if chunk.is_empty() => continue,
                Poll::Ready(Some(Ok(chunk))) => {
                    this.carry = Some(chunk);
                }
                Poll::Ready(Some(Err(err))) => {
                    if let Some(tx) = this.result.take() {
                        let _ = tx.send(err.to_string());
                    }
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
                // Clean end of body: the delivery future resolves Ok.
                Poll::Ready(None) => return Poll::Ready(Ok(StreamResult::Dropped)),
                Poll::Pending if finish => return Poll::Ready(Ok(StreamResult::Cancelled)),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<T> bindings::tina::http::client::HostWithStore<T> for crate::HostState
where
    T: 'static + Send,
{
    async fn send(
        store: &Accessor<T, crate::HostState>,
        request: Request,
    ) -> Result<Outcome, Error>
    {
        let method = match reqwest::Method::from_bytes(request.method.as_bytes()) {
            Ok(method) => method,
            Err(_) => {
                return Err(Error::Message(format!("invalid method {}", request.method)))
            }
        };
        let url = match reqwest::Url::parse(&request.url) {
            Ok(url) => url,
            Err(err) => {
                return Err(Error::Message(format!("invalid url {}: {err}", request.url)))
            }
        };

        // The capability document's network entries are the allowlist: an
        // allowed domain goes out now, an unknown one parks behind an
        // approval round-trip. Legacy launches keep the unrestricted
        // pre-merger behavior.
        let ask = store.with(|mut access| {
            match domain_verdict(&access.get().policy, &url) {
                DomainVerdict::Ask { reason } => Some(reason),
                DomainVerdict::Proceed => None,
            }
        });
        match ask {
            Some(reason) => {
                let id = store.with(|mut access| access.get().http_approvals.park(request));
                Ok(Outcome::Approval(ApprovalRequest {
                    id,
                    url: url.to_string(),
                    reason,
                }))
            }
            None => issue(store, request, method, url)
                .await
                .map(Outcome::Sent),
        }
    }

    async fn respond(
        store: &Accessor<T, crate::HostState>,
        approval: u64,
        allow: bool,
    ) -> Result<Outcome, Error> {
        let parked = store.with(|mut access| access.get().http_approvals.take(approval));
        match parked {
            Some(request) if allow => {
                // The parked record is authoritative; re-parse its method
                // and url exactly like `send` did.
                let method = match reqwest::Method::from_bytes(request.method.as_bytes()) {
                    Ok(method) => method,
                    Err(_) => {
                        return Err(Error::Message(format!(
                            "invalid method {}",
                            request.method
                        )))
                    }
                };
                let url = match reqwest::Url::parse(&request.url) {
                    Ok(url) => url,
                    Err(err) => {
                        return Err(Error::Message(format!(
                            "invalid url {}: {err}",
                            request.url
                        )))
                    }
                };
                issue(store, request, method, url)
                    .await
                    .map(Outcome::Sent)
            }
            Some(_) => Ok(Outcome::Denied(
                "the approval flow denied the request".to_string(),
            )),
            None => Err(Error::Message(format!("unknown approval id {approval}"))),
        }
    }
}

/// Sends an already-approved request and exposes the response: the head
/// plus the body as a `StreamReader` and delivery future - the handles
/// the Dart side lowers with the same stream machinery it uses for
/// `tina:process` output pipes.
async fn issue<T>(
    store: &Accessor<T, crate::HostState>,
    request: Request,
    method: reqwest::Method,
    url: reqwest::Url,
) -> Result<Sent, Error>
where
    T: 'static + Send,
{
    let client = store.with(|mut access| access.get().http_client());

    let mut outgoing = client.request(method, url);
    for (name, value) in &request.headers {
        let value = match reqwest::header::HeaderValue::from_str(value) {
            Ok(value) => value,
            Err(_) => {
                return Err(Error::Message(format!("invalid value for header {name}")))
            }
        };
        outgoing = outgoing.header(name, value);
    }
    if let Some(body) = request.body {
        outgoing = outgoing.body(body);
    }

    // One request per call, no retries: the engine's port semantics are
    // dart:io's - a failed connection surfaces as an error to the
    // caller, which decides what to do.
    let response = match outgoing.send().await {
        Ok(response) => response,
        Err(err) => {
            return Err(Error::Message(format!("request failed: {err}")));
        }
    };

    let head = bindings::tina::http::types::ResponseHead {
        status: response.status().as_u16(),
        headers: response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect(),
    };

    let (result_tx, result_rx) = oneshot::channel();
    let producer = HttpBodyProducer {
        stream: Box::pin(response.bytes_stream()),
        carry: None,
        result: Some(result_tx),
    };
    let made = store.with(|mut access| -> Result<(StreamReader<u8>, FutureReader<Result<(), Error>>), Error> {
        let stream = StreamReader::new(&mut access, producer)
            .map_err(failed_to_expose)?;
        let future = FutureReader::new(&mut access, async {
            wasmtime::error::Ok(match result_rx.await {
                // The producer only sends on a body transfer failure.
                Ok(message) => Err(Error::Message(message)),
                Err(_) => Ok(()),
            })
        })
        .map_err(failed_to_expose)?;
        Ok((stream, future))
    });
    let (body, delivered) = made?;
    Ok(Sent {
        head,
        body,
        delivered,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::CapabilityPolicy;

    fn legacy() -> CapabilityPolicy {
        CapabilityPolicy::from_legacy(Vec::new(), Vec::new())
    }

    fn document(domains: &[(&str, &str)], allow_all: bool) -> CapabilityPolicy {
        let ws = std::env::temp_dir().join(format!("tina-http-doc-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();
        let canonical = std::fs::canonicalize(&ws).unwrap().display().to_string();
        let entries: Vec<String> = domains
            .iter()
            .map(|(domain, origin)| format!("{{\"domain\": \"{domain}\", \"origin\": \"{origin}\"}}"))
            .collect();
        let json = format!(
            r#"{{
                "version": 1,
                "mode": "ask",
                "workspace": "{canonical}",
                "directories": {{
                    "read": [],
                    "write": [{{"path": "{canonical}", "guest": ".", "origin": "structural:workspace"}}]
                }},
                "network": {{
                    "allow_all": {allow_all},
                    "domains": [{}]
                }},
                "commands": {{
                    "default_verdict": "ask",
                    "rules": []
                }},
                "plugins": {{"grants": []}},
                "protected_paths": [],
                "preopens": [{{"guest": ".", "host": "{canonical}", "access": "rw", "origin": "structural:workspace"}}]
            }}"#,
            entries.join(","),
        );
        CapabilityPolicy::from_document_json(&json).expect("valid document")
    }

    fn verdict(policy: &CapabilityPolicy, target: &str) -> DomainVerdict {
        let parsed = reqwest::Url::parse(target).expect("a test url");
        domain_verdict(policy, &parsed)
    }

    fn assert_proceed(verdict: DomainVerdict) {
        match verdict {
            DomainVerdict::Proceed => {}
            DomainVerdict::Ask { reason } => {
                panic!("expected Proceed, got Ask: {reason}")
            }
        }
    }

    fn assert_ask(verdict: DomainVerdict, want: &str) {
        match verdict {
            DomainVerdict::Proceed => panic!("expected Ask for {want}, got Proceed"),
            DomainVerdict::Ask { reason } => assert!(reason.contains(want), "reason: {reason}"),
        }
    }

    #[test]
    fn legacy_mode_sends_everything() {
        assert_proceed(verdict(&legacy(), "https://anything.example.net/x"));
        assert_proceed(verdict(&legacy(), "http://localhost:9999/y"));
    }

    #[test]
    fn allow_all_document_sends_everything() {
        let policy = document(&[], true);
        assert_proceed(verdict(&policy, "https://anything.example.net/x"));
    }

    #[test]
    fn allowlisted_domains_send_and_unknown_domains_ask() {
        let policy = document(
            &[
                ("api.example.com", "provider:test"),
                ("localhost:8080", "setting:test"),
                ("*.services.example.com", "grant:g1"),
            ],
            false,
        );

        // Provider host, scheme-defaulted ports.
        assert_proceed(verdict(&policy, "https://api.example.com/v1"));
        assert_proceed(verdict(&policy, "http://api.example.com/v1"));
        // Explicit port entry: only that port.
        assert_proceed(verdict(&policy, "http://localhost:8080/sse"));
        assert_ask(
            verdict(&policy, "http://localhost:8081/sse"),
            "localhost",
        );
        // Wildcard: subdomains yes, the bare domain no.
        assert_proceed(verdict(&policy, "https://a.services.example.com/x"));
        assert_ask(
            verdict(&policy, "https://services.example.com/x"),
            "services.example.com",
        );
        // Unknown host asks, naming the domain.
        assert_ask(
            verdict(&policy, "https://telemetry.example.net/x"),
            "telemetry.example.net",
        );
    }

    #[test]
    fn parked_requests_round_trip() {
        let mut approvals = HttpApprovals::default();
        let request = Request {
            method: "GET".to_string(),
            url: "https://telemetry.example.net/x".to_string(),
            headers: Vec::new(),
            body: None,
        };
        let id = approvals.park(request);
        let taken = approvals.take(id).expect("parked request");
        assert_eq!(taken.url, "https://telemetry.example.net/x");
        assert!(approvals.take(id).is_none(), "a taken approval is gone");
        assert!(approvals.take(999).is_none());
    }
}
