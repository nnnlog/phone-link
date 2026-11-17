import Foundation
import Observation
import PhoneLinkEngine

private let log = AppLog(category: "call-control")

/// What a call-control banner says and offers.
public struct CallControlNotice: Equatable, Sendable {
    public enum Action: Equatable, Sendable {
        case openBluetoothSettings
        case pair
        case connect
        case tryAgain
        /// A dial failure: the banner only closes.
        case dismiss
    }

    public let title: String
    public let message: String
    public let action: Action

    public var actionTitle: String {
        switch action {
        case .openBluetoothSettings: String(localized: "Open Bluetooth Settings")
        case .pair: String(localized: "Begin Pairing")
        case .connect: String(localized: "Connect")
        case .tryAgain: String(localized: "Try Again")
        case .dismiss: String(localized: "OK")
        }
    }
}

/// The section's waits and the clock its popups count with.
struct CallControlTiming: Sendable {
    /// An incoming call's popup closes after this long.
    var popupLifetime: Duration

    static let live = CallControlTiming(popupLifetime: .seconds(60))
}

/// Call control: the phone's calls placed, answered, declined and ended from this Mac over Bluetooth,
/// with every call's audio on the phone.
///
/// The model reads the engine's call-control state and current calls, and drives what shows them: the
/// incoming-call popup (every ringing call the user has not closed, for at most a minute, with the
/// Mac's ring), the call window (the calls placed or answered here), and the banners of the Calls
/// view. The first switch-on on this Mac explains first that the calls stay on the phone.
@MainActor
@Observable
public final class CallControlModel {
    /// `nil` until the engine has started.
    public private(set) var state: CallControlState?
    /// The phone's current calls.
    public private(set) var calls: [ActiveCall] = []
    /// The last dial's failure, until the user closes it.
    public var dialNotice: CallControlNotice?
    /// The failure of an action the user started.
    public var alert: MessagesAlert?
    /// A *Connect*, *Try Again* or *Begin Pairing* runs.
    public private(set) var isConnecting = false
    /// A dial runs.
    public private(set) var isDialing = false

    /// Opens or closes the incoming-call popup (set by the app).
    @ObservationIgnored public var presentPopup: (Bool) -> Void = { _ in }
    /// Opens or closes the call window (set by the app).
    @ObservationIgnored public var presentCallWindow: (Bool) -> Void = { _ in }

    /// Incoming calls the user closed, or whose popup ran out.
    @ObservationIgnored private var hidden: Set<UInt64> = []
    @ObservationIgnored private var expiries: [UInt64: Task<Void, Never>] = [:]
    @ObservationIgnored private var generations = (state: 0, calls: 0)
    @ObservationIgnored private var popupShown = false
    /// The primary call the call window was last opened for.
    @ObservationIgnored private var windowCall: UInt64?

    private let api: any CallingApiProtocol
    /// Sends a text to a number through messaging.
    private let sendText: @Sendable (_ number: String, _ text: String) async throws -> Void
    private let navigator: AppNavigator
    private let ringer: any CallRinging
    private let reads: any MessagesReading
    private let timing: CallControlTiming
    private let defaults: UserDefaults
    /// Whether the phone lets this Mac send messages, for the popup's *Message* (set by the app).
    @ObservationIgnored public var canMessage: () -> Bool = { false }

    public convenience init(
        api: any CallingApiProtocol,
        messaging: any MessagingApiProtocol,
        hub: EngineEventHub,
        navigator: AppNavigator,
        ringer: any CallRinging
    ) {
        self.init(
            api: api,
            sendText: { number, text in
                _ = try await messaging.send(target: .newConversation(recipients: [number]), text: text, attachments: [])
            },
            hub: hub,
            navigator: navigator,
            ringer: ringer,
            reads: MessagesBackgroundReads(),
            timing: .live,
            defaults: .standard
        )
    }

    init(
        api: any CallingApiProtocol,
        sendText: @escaping @Sendable (_ number: String, _ text: String) async throws -> Void,
        hub: EngineEventHub,
        navigator: AppNavigator,
        ringer: any CallRinging,
        reads: any MessagesReading,
        timing: CallControlTiming,
        defaults: UserDefaults
    ) {
        self.api = api
        self.sendText = sendText
        self.navigator = navigator
        self.ringer = ringer
        self.reads = reads
        self.timing = timing
        self.defaults = defaults

        hub.onEvent { [weak self] event in
            self?.handle(event)
        }
        hub.onStart { [weak self] in
            self?.reloadState()
            self?.reloadCalls()
        }
    }

    // MARK: - What the views show

    /// The phone can have call control.
    public var isOffered: Bool {
        state?.offered == true
    }

    /// "Show call history on this Mac", which call control needs.
    public var hasCallHistory: Bool {
        state?.pcConsent == true
    }

    /// "Make and answer calls on this Mac".
    public var isEnabled: Bool {
        state?.enabled == true
    }

    public var status: CallControlStatus {
        state?.status ?? .off
    }

    /// A call can be placed now.
    public var canDial: Bool {
        status == .ready && !isDialing
    }

    /// The first switch-on on this Mac explains first.
    public var needsIntro: Bool {
        !defaults.bool(forKey: PreferenceKeys.callControlIntroShown)
    }

    /// The ringing calls whose popup shows.
    public var popupCalls: [ActiveCall] {
        calls.filter { $0.status == .incoming && !hidden.contains($0.id) }
    }

    /// The call window's main call.
    public var primaryCall: ActiveCall? {
        calls.first { $0.role == .primary }
    }

    /// The call window's other call.
    public var secondaryCall: ActiveCall? {
        calls.first { $0.role == .secondary }
    }

    /// What the connection needs from the user; `nil` while it is ready, getting ready or off.
    public var connectionNotice: CallControlNotice? {
        guard isEnabled && hasCallHistory else { return nil }
        switch status {
        case .bluetoothOff:
            return CallControlNotice(
                title: String(localized: "Bluetooth Is Off on This Mac"),
                message: String(localized: "Turn on Bluetooth to make and answer calls on this Mac."),
                action: .openBluetoothSettings
            )
        case .notPaired:
            return CallControlNotice(
                title: String(localized: "Phone Not Paired with This Mac"),
                message: String(localized: "Keep your phone nearby with Bluetooth on, then pair it with this Mac."),
                action: .pair
            )
        case .notConnected:
            return CallControlNotice(
                title: String(localized: "Phone Not Connected"),
                message: String(localized: "Keep your phone nearby with Bluetooth on, then connect again."),
                action: .connect
            )
        case .connectionFailed:
            return CallControlNotice(
                title: String(localized: "Couldn't Connect to Your Phone"),
                message: String(localized: "Turn Bluetooth off and back on, on both your phone and this Mac, then try again."),
                action: .tryAgain
            )
        case .off, .starting, .connecting, .ready:
            return nil
        }
    }

    /// The connection's state in a few words, for Settings.
    public var statusText: String {
        switch status {
        case .off: String(localized: "Off")
        case .starting, .connecting: String(localized: "Connecting…")
        case .bluetoothOff: String(localized: "Bluetooth is off")
        case .notPaired: String(localized: "Not paired")
        case .connectionFailed: String(localized: "Couldn't connect")
        case .notConnected: String(localized: "Not connected")
        case .ready: String(localized: "Connected")
        }
    }

    // MARK: - Switching on

    /// Remembers that the user read why calls stay on the phone.
    public func acknowledgeIntro() {
        defaults.set(true, forKey: PreferenceKeys.callControlIntroShown)
    }

    /// "Make and answer calls on this Mac".
    public func setEnabled(_ enabled: Bool) {
        do {
            try api.setControlEnabled(enabled: enabled)
            reloadState()
        } catch {
            log.error("call control not switched: \(error: error)")
            alert = MessagesAlert(title: String(localized: "Couldn't Turn On Calls"), message: UserText.describe(error))
        }
    }

    /// The action of `notice`.
    public func act(on notice: CallControlNotice) async {
        switch notice.action {
        case .openBluetoothSettings:
            navigator.openBluetoothSettings()
        case .pair:
            await connect(pairing: true)
        case .connect, .tryAgain:
            await connect(pairing: false)
        case .dismiss:
            dialNotice = nil
        }
    }

    private func connect(pairing: Bool) async {
        guard !isConnecting else { return }
        isConnecting = true
        defer { isConnecting = false }

        do {
            let reached = pairing ? try await api.pairForControl() : try await api.connectControl()
            log.notice("call control connect finished: \(String(describing: reached), privacy: .public)")
        } catch {
            log.error("call control did not connect: \(error: error)")
            alert = MessagesAlert(title: String(localized: "Couldn't Connect to Your Phone"), message: UserText.describe(error))
        }
        reloadState()
    }

    // MARK: - Calls

    /// Places a call to `number` on the phone.
    public func dial(_ number: String) async {
        guard canDial else { return }
        isDialing = true
        dialNotice = nil
        defer { isDialing = false }

        do {
            try await api.dial(number: number)
        } catch let CallControlError.DialFailed(kind) {
            dialNotice = Self.notice(for: kind)
        } catch {
            log.error("call not placed: \(error: error)")
            alert = MessagesAlert(title: String(localized: "Call Not Placed"), message: UserText.describe(error))
        }
        reloadCalls()
    }

    public func answer(_ call: ActiveCall) async {
        await run(String(localized: "Couldn't Answer Call")) { try await $0.answer(id: call.id) }
    }

    public func decline(_ call: ActiveCall) async {
        await run(String(localized: "Couldn't Decline Call")) { try await $0.decline(id: call.id) }
    }

    public func end(_ call: ActiveCall) async {
        await run(String(localized: "Couldn't End Call")) { try await $0.endCall(id: call.id) }
    }

    /// Resumes the call window's held call and holds the other.
    public func swap() async {
        await run(String(localized: "Couldn't Swap Calls")) { try await $0.swap() }
    }

    /// Hides `call`'s popup; the call keeps ringing on the phone.
    public func hidePopup(_ call: ActiveCall) {
        hidden.insert(call.id)
        updatePresentation()
    }

    /// The popup's *Message* is offered: the phone lets this Mac send messages and the caller has a
    /// number.
    public func canReply(to call: ActiveCall) -> Bool {
        !call.number.isEmpty && canMessage()
    }

    /// Declines `call` and sends `text` to its number.
    public func reply(to call: ActiveCall, with text: String) async {
        await decline(call)
        do {
            try await sendText(call.number, text)
        } catch {
            log.error("call reply not sent: \(error: error)")
            alert = MessagesAlert(title: String(localized: "Message Not Sent"), message: UserText.describe(error))
        }
    }

    /// Declines `call` and opens Messages addressed to its number.
    public func replyInMessages(to call: ActiveCall) async {
        await decline(call)
        navigator.showMessage(to: MessagesRecipient(address: call.number, name: call.name))
    }

    /// The ready-made replies of the incoming-call popup's *Message* menu.
    public static let cannedReplies = [
        String(localized: "Sorry, I can't talk right now."),
        String(localized: "I'll call you back in a minute."),
        String(localized: "I'm busy. Call me back later."),
    ]

    private func run(_ failureTitle: String, _ work: @escaping (any CallingApiProtocol) async throws -> Void) async {
        do {
            try await work(api)
        } catch {
            log.error("call operation failed: \(error: error)")
            alert = MessagesAlert(title: failureTitle, message: UserText.describe(error))
        }
        reloadCalls()
    }

    static func notice(for kind: DialFailureKind) -> CallControlNotice {
        switch kind {
        case .noNetwork:
            CallControlNotice(
                title: String(localized: "Call Not Placed"),
                message: String(localized: "Your phone isn't connected to a mobile network. Check its SIM card and signal, or place the call on your phone."),
                action: .dismiss
            )
        case .airplaneMode:
            CallControlNotice(
                title: String(localized: "Call Not Placed"),
                message: String(localized: "Your phone is in Airplane Mode. Turn off Airplane Mode on your phone, then try again."),
                action: .dismiss
            )
        case .failed:
            CallControlNotice(
                title: String(localized: "Call Not Placed"),
                message: String(localized: "Turn Bluetooth off and back on, on both your phone and this Mac, then try again. If the call still isn't placed, place it on your phone."),
                action: .dismiss
            )
        }
    }

    // MARK: - Engine events and loading

    private func handle(_ event: EngineEvent) {
        switch event {
        case let .calling(change):
            switch change {
            case .controlChanged, .stateChanged:
                reloadState()
            case .activeCalls:
                reloadCalls()
            case .historyChanged:
                break
            }
        case .resync(.calling), .phoneChanged:
            reloadState()
            reloadCalls()
        case .resync(.contacts), .contacts(.recordsChanged):
            // Contacts name the calls.
            reloadCalls()
        default:
            break
        }
    }

    private func reloadState() {
        generations.state += 1
        let generation = generations.state

        reads.read({ [api] in try api.controlState() }) { [weak self] result in
            guard let self, generation == self.generations.state else { return }
            switch result {
            case let .success(loaded):
                state = loaded
            case let .failure(error):
                record(error, "call control state not read")
            }
        }
    }

    private func reloadCalls() {
        generations.calls += 1
        let generation = generations.calls

        reads.read({ [api] in try api.activeCalls() }) { [weak self] result in
            guard let self, generation == self.generations.calls else { return }
            switch result {
            case let .success(loaded):
                calls = loaded
                forgetEnded()
                updatePresentation()
            case let .failure(error):
                record(error, "current calls not read")
            }
        }
    }

    /// Drops what the model keeps about calls that ended.
    private func forgetEnded() {
        let ids = Set(calls.map(\.id))
        hidden.formIntersection(ids)
        for (id, expiry) in expiries where !ids.contains(id) {
            expiry.cancel()
            expiries[id] = nil
        }
    }

    /// Opens or closes the popup and the call window, rings while the popup shows, and starts each new
    /// popup call's lifetime.
    private func updatePresentation() {
        for call in popupCalls where expiries[call.id] == nil {
            let lifetime = timing.popupLifetime
            expiries[call.id] = Task { [weak self] in
                try? await Task.sleep(for: lifetime)
                guard !Task.isCancelled else { return }
                self?.hidden.insert(call.id)
                self?.updatePresentation()
            }
        }

        let popup = !popupCalls.isEmpty
        if popup != popupShown {
            popupShown = popup
            popup ? ringer.start() : ringer.stop()
            presentPopup(popup)
        }

        // A new primary call opens the window again, also after the user closed it.
        let primary = primaryCall?.id
        if primary != windowCall {
            let wasShown = windowCall != nil
            windowCall = primary
            if primary != nil || wasShown {
                presentCallWindow(primary != nil)
            }
        }
    }

    private func record(_ error: any Error, _ subject: StaticString) {
        if case FeatureApiError.NotStarted? = error as? FeatureApiError { return }
        log.error("\(subject.description, privacy: .public): \(error: error)")
    }
}
