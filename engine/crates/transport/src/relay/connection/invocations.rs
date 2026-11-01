//! Hub invocations and their completions.

use super::{CompletionReceiver, HubConnection, InvokeError};
use phonelink_core::codec::hub;
use phonelink_core::types::{HubSendResult, HubState, TraceContext};
use rmpv::Value;

impl HubConnection {
    /// Queues an invocation on the live session and returns its completion waiter; the outbound hub
    /// methods' `NotConnected` pre-check applies.
    pub(crate) fn start_invoke(&self, target: &str, arguments: Vec<Value>) -> Result<CompletionReceiver, InvokeError> {
        let session = {
            let inner = self.inner();
            if inner.state != HubState::Connected {
                return Err(InvokeError::NotConnected);
            }
            inner.session.clone().expect("a Connected hub connection always has a live session")
        };

        session.invoke(&self.shared, self.params.env, target, arguments)
    }

    /// One hub invocation awaited for its completion, mapped to a [`HubSendResult`].
    /// `BrokenCircuitError` is not produced: an invocation over the WebSocket cannot carry an HTTP
    /// 429.
    pub(crate) async fn invoke(&self, target: &str, arguments: Vec<Value>) -> HubSendResult {
        let receiver = match self.start_invoke(target, arguments) {
            Ok(receiver) => receiver,
            Err(InvokeError::NotConnected) => return HubSendResult::NotConnected,
            Err(error) => return self.failure(target, &error),
        };

        match receiver.await {
            Ok(Ok(())) => HubSendResult::Success,
            Ok(Err(error)) => self.failure(target, &error),
            Err(_) => self.failure(target, &InvokeError::ConnectionLost),
        }
    }

    pub(super) fn failure(&self, target: &str, error: &InvokeError) -> HubSendResult {
        let result =
            if self.shared.internet_available() { HubSendResult::Failure } else { HubSendResult::InternetError };
        tracing::debug!(env = %self.params.env, call = target, ?error, ?result, "hub invocation failed");
        result
    }

    /// `SendConnectedAsync(targetId)` with a fresh trace context whose `TraceState` is exactly
    /// `{ypScenarioId: SendConnectedMessage, ypTriggerId: ConnectingWithPartner}`.
    pub(crate) fn start_send_connected(&self, target_id: &str) -> Result<CompletionReceiver, InvokeError> {
        let ctx = TraceContext::new_root(hub::send_connected_trace_state());
        self.start_invoke(hub::method::SEND_CONNECTED, hub::send_connected_arguments(&ctx, target_id))
    }

    pub(crate) async fn send_connected(&self, target_id: &str) -> HubSendResult {
        let ctx = TraceContext::new_root(hub::send_connected_trace_state());
        self.invoke(hub::method::SEND_CONNECTED, hub::send_connected_arguments(&ctx, target_id)).await
    }
}
