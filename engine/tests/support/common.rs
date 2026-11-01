//! Small helpers every world uses.

use std::future::Future;
use std::pin::Pin;
use std::task::Poll;
use std::time::Duration;

use phonelink_engine::codec::valueset::{self, ValueSet};
use phonelink_engine::seams::PlatformRequest;

/// The `ValueSet` an App request carries.
pub fn payload(request: &PlatformRequest) -> ValueSet {
    valueset::decode(&request.message.payload).expect("an App request carries a ValueSet")
}

/// Whether `cond` holds within 10 s, checked every 10 ms (of paused time, in a paused test).
pub async fn eventually(mut cond: impl FnMut() -> bool) -> bool {
    for _ in 0..1000 {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cond()
}

/// Polls `future` once: its steps up to its first wait have run when this returns, so a waiter it
/// registers is registered.
pub async fn poll_once<F: Future + Unpin>(future: &mut F) -> Poll<F::Output> {
    std::future::poll_fn(|cx| Poll::Ready(Pin::new(&mut *future).poll(cx))).await
}

/// Runs `attempt` every 10 ms until it gives `Some`, for at most 10 s (of paused time, in a paused
/// test): for a state that only a call can observe, where a call made too early changes nothing.
pub async fn retry_until<T, Fut: Future<Output = Option<T>>>(mut attempt: impl FnMut() -> Fut) -> Option<T> {
    for _ in 0..1000 {
        if let Some(value) = attempt().await {
            return Some(value);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    None
}

/// Asserts that `value` matches `pattern` (with an optional guard), showing the value when not.
#[macro_export]
macro_rules! assert_matches {
    ($value:expr, $pattern:pat $(if $guard:expr)? $(,)?) => {
        match $value {
            $pattern $(if $guard)? => {}
            other => panic!("{other:?} does not match {}", stringify!($pattern $(if $guard)?)),
        }
    };
    ($value:expr, $pattern:pat $(if $guard:expr)?, $($message:tt)+) => {
        match $value {
            $pattern $(if $guard)? => {}
            other => panic!("{other:?} does not match {}: {}", stringify!($pattern $(if $guard)?), format!($($message)+)),
        }
    };
}
