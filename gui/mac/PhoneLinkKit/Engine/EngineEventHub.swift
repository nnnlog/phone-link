import Foundation
import PhoneLinkEngine

/// Fans the engine's events out to the app's models on the main actor.
///
/// The engine calls its `EventSink` on its own threads; `EventBridge` hops to the main actor and
/// posts here. Models register once, at construction, and live as long as the app, so the hub keeps
/// its handlers strongly.
@MainActor
public final class EngineEventHub {
    private var eventHandlers: [(EngineEvent) -> Void] = []
    private var startHandlers: [() -> Void] = []

    public init() {}

    /// Calls `handler` with every engine event, in order.
    public func onEvent(_ handler: @escaping @MainActor (EngineEvent) -> Void) {
        eventHandlers.append(handler)
    }

    /// Calls `handler` after every successful `Engine.start()`: the features exist from then on, so
    /// models load what they show.
    public func onStart(_ handler: @escaping @MainActor () -> Void) {
        startHandlers.append(handler)
    }

    public func post(_ event: EngineEvent) {
        for handler in eventHandlers {
            handler(event)
        }
    }

    public func postStarted() {
        for handler in startHandlers {
            handler()
        }
    }
}

/// The engine's `EventSink`: hands every event to the hub on the main thread, in the order the
/// engine raised them (the main queue is FIFO; separate `Task`s would not promise that).
public final class EventBridge: EventSink {
    private let hub: EngineEventHub

    public init(hub: EngineEventHub) {
        self.hub = hub
    }

    public func onEvent(event: EngineEvent) {
        let hub = self.hub
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                hub.post(event)
            }
        }
    }
}
