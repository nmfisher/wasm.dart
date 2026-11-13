//! The `tina:http` host import: the engine's HTTP port served by the host
//! with `reqwest`, following the custom-import pattern the runner already
//! uses for `wasm:dart/trace` and `wasm:dart/timeline`.
//!
//! The interface (see `wit/tina_http.wit`) has one async function,
//! `request`, that completes with the response head plus the body as a
//! component-model byte stream and a delivery future - the exact shape of
//! `wasi:cli/stdin.read-via-stream`, lowered through the same
//! `StreamReader`/`FutureReader` machinery wasmtime-wasi uses for stdio.

use std::{
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

use bindings::tina::http::types::{Error, Request, ResponseHead};

// The store state doubles as the host implementation: `HasData` exposes it
// as its own view (like `HasSelf`, but on the concrete state so the client
// trait's `HostWithStore`/`Host` bounds are all satisfied locally), and the
// empty `Host` marker is what the generated linker machinery wraps in
// `&mut` when it calls the interface's functions.
impl wasmtime::component::HasData for crate::HostState {
    type Data<'a> = &'a mut crate::HostState;
}

impl bindings::tina::http::client::Host for crate::HostState {}

/// Adds the `tina:http/client@1.0.0` instance to `linker`. Components that
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
    ) -> Result<(ResponseHead, StreamReader<u8>, FutureReader<Result<(), Error>>), Error>
    {
        let client = store.with(|mut access| access.get().http_client());
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

        let head = ResponseHead {
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
        let (stream, future) = made?;
        Ok((head, stream, future))
    }
}
