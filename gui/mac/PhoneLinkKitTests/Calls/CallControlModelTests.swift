import Foundation
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

// MARK: - Fakes

/// A value shared between the test and the fake's async methods, which run off the main actor.
private final class Box<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Value

    init(_ value: Value) {
        self.value = value
    }

    @discardableResult
    func with<T>(_ body: (inout Value) throws -> T) rethrows -> T {
        lock.lock()
        defer { lock.unlock() }
        return try body(&value)
    }
}

@MainActor
private final class InlineReads: MessagesReading {
    func read<T: Sendable>(
        _ work: @escaping @Sendable () throws -> T,
        then apply: @escaping @MainActor (Result<T, any Error>) -> Void
    ) {
        apply(Result { try work() })
    }
}

private struct ControlData {
    var state = CallControlState(offered: true, pcConsent: true, enabled: false, status: .off)
    var calls: [ActiveCall] = []
    var operations: [String] = []
    var dialFailure: DialFailureKind?
    var sent: [(String, String)] = []
}

private final class FakeControl: CallingApiProtocol, @unchecked Sendable {
    let data = Box(ControlData())

    func controlState() throws -> CallControlState { data.with { $0.state } }
    func activeCalls() throws -> [ActiveCall] { data.with { $0.calls } }

    func setControlEnabled(enabled: Bool) throws {
        data.with { d in
            d.operations.append("enabled \(enabled)")
            d.state.enabled = enabled
            d.state.status = enabled ? .ready : .off
        }
    }

    func connectControl() async throws -> CallControlStatus {
        data.with { d in
            d.operations.append("connect")
            d.state.status = .ready
            return .ready
        }
    }

    func pairForControl() async throws -> CallControlStatus {
        data.with { d in
            d.operations.append("pair")
            d.state.status = .ready
            return .ready
        }
    }

    func dial(number: String) async throws {
        let failure = data.with { d in
            d.operations.append("dial \(number)")
            return d.dialFailure
        }
        if let failure { throw CallControlError.DialFailed(kind: failure) }
    }

    func answer(id: UInt64) async throws { data.with { $0.operations.append("answer \(id)") } }
    func decline(id: UInt64) async throws { data.with { $0.operations.append("decline \(id)") } }
    func endCall(id: UInt64) async throws { data.with { $0.operations.append("end \(id)") } }
    func swap() async throws { data.with { $0.operations.append("swap") } }

    func state() throws -> CallingState { preconditionFailure("not called: \(#function)") }
    func history() throws -> [CallHistoryItem] { preconditionFailure("not called: \(#function)") }
    func markSeen() throws { preconditionFailure("not called: \(#function)") }
    func setPcConsent(consent: Bool) throws { preconditionFailure("not called: \(#function)") }
    func refreshIfEmpty() async throws -> RequestOutcome? { preconditionFailure("not called: \(#function)") }
    func requestPermission() async throws -> CallsPermissionResult { preconditionFailure("not called: \(#function)") }
}

@MainActor
private final class FakeRinger: CallRinging {
    var ringing = false
    func start() { ringing = true }
    func stop() { ringing = false }
}

private func activeCall(
    _ id: UInt64,
    _ status: ActiveCallStatus,
    role: ActiveCallRole = .untracked,
    number: String = "+15550100",
    name: String? = "Ada"
) -> ActiveCall {
    ActiveCall(
        id: id, number: number, name: name, contactId: nil, photoContactId: nil, incoming: true, status: status,
        waiting: false, talkingSinceMs: nil, role: role
    )
}

@MainActor
private final class World {
    let api = FakeControl()
    let hub = EngineEventHub()
    let navigator = AppNavigator()
    let ringer = FakeRinger()
    let defaults: UserDefaults
    let sent = Box<[(String, String)]>([])
    var model: CallControlModel!
    var popup: [Bool] = []
    var window: [Bool] = []

    init(popupLifetime: Duration = .seconds(60)) {
        let suite = "CallControlModelTests.\(UUID().uuidString)"
        defaults = UserDefaults(suiteName: suite) ?? .standard
        defaults.removePersistentDomain(forName: suite)

        let sent = sent
        model = CallControlModel(
            api: api,
            sendText: { number, text in sent.with { $0.append((number, text)) } },
            hub: hub,
            navigator: navigator,
            ringer: ringer,
            reads: InlineReads(),
            timing: CallControlTiming(popupLifetime: popupLifetime),
            defaults: defaults
        )
        model.presentPopup = { [unowned self] in popup.append($0) }
        model.presentCallWindow = { [unowned self] in window.append($0) }
        model.canMessage = { true }
        hub.postStarted()
    }

    func calls(_ calls: [ActiveCall]) {
        api.data.with { $0.calls = calls }
        hub.post(.calling(change: .activeCalls))
    }
}

// MARK: - Tests

@MainActor
struct CallControlModelTests {
    @Test func theFirstSwitchOnExplainsOnceOnThisMac() {
        let world = World()
        #expect(world.model.needsIntro)

        world.model.acknowledgeIntro()
        world.model.setEnabled(true)

        #expect(!world.model.needsIntro)
        #expect(world.model.isEnabled)
        #expect(world.api.data.with { $0.operations } == ["enabled true"])
    }

    @Test func aRingingCallOpensThePopupAndRingsUntilItIsAnswered() async {
        let world = World()
        world.calls([activeCall(1, .incoming)])
        #expect(world.popup == [true])
        #expect(world.ringer.ringing)
        #expect(world.model.popupCalls.map(\.id) == [1])

        await world.model.answer(world.model.popupCalls[0])
        world.calls([activeCall(1, .talking, role: .primary)])

        #expect(world.popup == [true, false])
        #expect(!world.ringer.ringing)
        #expect(world.window == [true])
        #expect(world.model.primaryCall?.id == 1)
        #expect(world.api.data.with { $0.operations } == ["answer 1"])
    }

    @Test func hidingThePopupLeavesTheCallRinging() {
        let world = World()
        world.calls([activeCall(1, .incoming)])
        world.model.hidePopup(world.model.popupCalls[0])

        #expect(world.popup == [true, false])
        #expect(world.model.calls.count == 1)
        #expect(world.api.data.with { $0.operations }.isEmpty)
    }

    @Test func thePopupRunsOutAfterItsLifetime() async {
        let world = World(popupLifetime: .milliseconds(10))
        world.calls([activeCall(1, .incoming)])
        for _ in 0..<200 where world.model.popupCalls.count == 1 {
            try? await Task.sleep(for: .milliseconds(5))
        }
        #expect(world.model.popupCalls.isEmpty)
        #expect(world.popup == [true, false])
    }

    @Test func aReplyDeclinesThenSendsTheText() async {
        let world = World()
        world.calls([activeCall(3, .incoming)])

        await world.model.reply(to: world.model.popupCalls[0], with: CallControlModel.cannedReplies[0])

        #expect(world.api.data.with { $0.operations } == ["decline 3"])
        #expect(world.sent.with { $0.map(\.0) } == ["+15550100"])
        #expect(world.sent.with { $0.map(\.1) } == [CallControlModel.cannedReplies[0]])
    }

    @Test func aPrivateNumberGetsNoReply() {
        let world = World()
        #expect(!world.model.canReply(to: activeCall(1, .incoming, number: "")))
        #expect(world.model.canReply(to: activeCall(1, .incoming)))
    }

    @Test func aRefusedDialShowsTheNoticeForThePhonesState() async {
        let world = World()
        world.model.setEnabled(true)
        world.api.data.with { $0.dialFailure = .noNetwork }

        await world.model.dial("5550101")

        #expect(world.model.dialNotice?.title == "Call Not Placed")
        #expect(world.model.dialNotice?.message.hasPrefix("Your phone isn't connected to a mobile network.") == true)
        #expect(world.model.dialNotice?.action == .dismiss)
        await world.model.act(on: world.model.dialNotice!)
        #expect(world.model.dialNotice == nil)
    }

    @Test func eachConnectionStateNeedingTheUserHasItsAction() async {
        let world = World()
        world.model.setEnabled(true)
        let cases: [(CallControlStatus, CallControlNotice.Action?)] = [
            (.ready, nil), (.connecting, nil), (.bluetoothOff, .openBluetoothSettings), (.notPaired, .pair),
            (.notConnected, .connect), (.connectionFailed, .tryAgain),
        ]
        for (status, action) in cases {
            world.api.data.with { $0.state.status = status }
            world.hub.post(.calling(change: .controlChanged))
            #expect(world.model.connectionNotice?.action == action, "\(status)")
        }

        await world.model.act(on: world.model.connectionNotice!)
        #expect(world.api.data.with { $0.operations }.last == "connect")
        #expect(world.model.connectionNotice == nil)
    }

    @Test func noNoticeWhileCallControlIsOff() {
        let world = World()
        world.api.data.with { $0.state.status = .notConnected }
        world.hub.post(.calling(change: .controlChanged))
        #expect(world.model.connectionNotice == nil)
    }
}
