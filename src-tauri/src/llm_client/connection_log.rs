//! Logs each connection the shared HTTP client opens, and each attempt that
//! fails or is abandoned.
//!
//! reqwest doesn't report a pooled connection closing (an HTTP/2 ping timeout,
//! the server closing it, or the pool's idle timeout), so a new connection to a
//! host that was just warmed up is the sign that the previous one closed.

use super::{error_source_chain, sanitized_url_for_log};
use log::{debug, warn};
use std::error::Error as StdError;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

type BoxError = Box<dyn StdError + Send + Sync>;

/// Tower layer for `reqwest::ClientBuilder::connector_layer`.
#[derive(Clone)]
pub(super) struct ConnectionLogLayer;

impl<S> tower_layer::Layer<S> for ConnectionLogLayer {
    type Service = ConnectionLog<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ConnectionLog { inner }
    }
}

#[derive(Clone)]
pub(super) struct ConnectionLog<S> {
    inner: S,
}

impl<S, Req> tower_service::Service<Req> for ConnectionLog<S>
where
    S: tower_service::Service<Req, Error = BoxError>,
    S::Response: Send + 'static,
    S::Future: Send + 'static,
    Req: Debug,
{
    type Response = S::Response;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, BoxError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Req) -> Self::Future {
        let mut attempt = Attempt {
            destination: destination(&request),
            started: Instant::now(),
            finished: false,
        };
        let connecting = self.inner.call(request);
        Box::pin(async move {
            let result = connecting.await;
            attempt.finished = true;
            match &result {
                Ok(_) => debug!(
                    "Opened HTTP connection to {} in {:.2?}",
                    attempt.destination,
                    attempt.started.elapsed()
                ),
                Err(error) => {
                    let mut causes = vec![error.to_string()];
                    causes.extend(error_source_chain(&**error));
                    warn!(
                        "Failed to open HTTP connection to {} after {:.2?}: {}",
                        attempt.destination,
                        attempt.started.elapsed(),
                        causes.join(" -> ")
                    );
                }
            }
            result
        })
    }
}

/// One connection attempt. reqwest's `connect_timeout` wraps this layer and
/// drops the attempt when it fires, so a drop before it finishes is logged as
/// well; the request being cancelled drops it the same way.
struct Attempt {
    destination: String,
    started: Instant,
    finished: bool,
}

impl Drop for Attempt {
    fn drop(&mut self) {
        if !self.finished {
            warn!(
                "Gave up opening HTTP connection to {} after {:.2?} (timed out or cancelled)",
                self.destination,
                self.started.elapsed()
            );
        }
    }
}

/// reqwest's connector request type is sealed, but its Debug output wraps the
/// destination URI, e.g. `Unnameable(https://api.openai.com/)`.
fn destination(request: &impl Debug) -> String {
    let text = format!("{request:?}");
    text.split_once('(')
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .map(sanitized_url_for_log)
        .unwrap_or_else(|| "<unknown host>".to_string())
}
