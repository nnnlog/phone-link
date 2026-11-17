import Foundation
import Testing
import PhoneLinkEngine
@testable import PhoneLinkKit

@MainActor
@Suite struct DeviceStatusModelTests {
    private let api = DeviceStatusApiFake()
    private let hub = EngineEventHub()
    private let alerts = AlertSpy()
    private let settings = SettingsOpenCounter()
    private let model: DeviceStatusModel

    init() {
        let alerts = alerts
        let settings = settings
        model = DeviceStatusModel(
            api: api,
            hub: hub,
            present: { await alerts.present($0) },
            openSettings: { settings.count += 1 }
        )
    }

    private func start(_ status: PhoneStatus = makeStatus()) {
        api.update { $0.status = .success(status) }
        hub.postStarted()
    }

    // MARK: - Loading

    @Test func theStatusLoadsOnStartAndOnEveryChange() {
        hub.postStarted()
        #expect(model.status == nil)
        #expect(model.lines.isEmpty)

        start()
        #expect(model.status == makeStatus())

        api.update { $0.status = .success(makeStatus(wifi: .notConnected)) }
        hub.post(.deviceStatus(change: .changed))
        #expect(model.status?.wifi == .notConnected)

        api.update { $0.status = .success(makeStatus(live: false)) }
        hub.post(.phoneSession(connected: false, failure: "partner disconnect"))
        #expect(model.status?.live == false)

        api.update { $0.status = .success(makeStatus(bluetooth: .off)) }
        hub.post(.resync(feature: .notifications))
        #expect(model.status?.bluetooth == .on)

        hub.post(.resync(feature: .deviceStatus))
        #expect(model.status?.bluetooth == .off)
    }

    @Test func aStatusTheEngineCannotReadKeepsTheLastOne() {
        start()
        api.update { $0.status = .failure(.Store(message: "disk")) }
        hub.post(.deviceStatus(change: .changed))
        #expect(model.status == makeStatus())

        api.update { $0.status = .failure(.NotStarted) }
        hub.post(.accountRemoved)
        #expect(model.status == nil)
    }

    // MARK: - Lines and controls

    @Test func theLinesShowWhatIsKnown() {
        start()
        #expect(model.lines == [
            DeviceStatusLine(.battery, "78% · Charging", "battery.100percent.bolt"),
            DeviceStatusLine(.cellular, "SKT", "cellularbars", variableValue: 0.75),
            DeviceStatusLine(.wifi, "Wi-Fi", "wifi", variableValue: 1),
            DeviceStatusLine(.bluetooth, "Bluetooth On", "antenna.radiowaves.left.and.right"),
        ])
    }

    @Test func unknownItemsAreHiddenAndKnownOnesNamed() {
        start(makeStatus(
            battery: .level(percent: 30, charging: false, powerSave: true),
            cellular: .airplaneMode,
            wifi: .unknown,
            bluetooth: .unknown,
            mobileData: false,
            metered: true
        ))
        #expect(model.lines == [
            DeviceStatusLine(.battery, "30% · Battery Saver", "battery.25percent"),
            DeviceStatusLine(.cellular, "Airplane Mode", "airplane"),
            DeviceStatusLine(.mobileData, "Mobile Data Off", "arrow.up.arrow.down"),
            DeviceStatusLine(.metered, "Metered Connection", "gauge.with.dots.needle.33percent"),
        ])
    }

    @Test(arguments: [
        (CellularState.noSim, DeviceStatusLine(.cellular, "No SIM", "simcard")),
        (.bars(level: 5, max: 5, operator: nil, fourBarGlyphs: false), DeviceStatusLine(.cellular, "Mobile Network", "cellularbars", variableValue: 1)),
        (.bars(level: 0, max: 4, operator: "", fourBarGlyphs: true), DeviceStatusLine(.cellular, "Mobile Network", "cellularbars", variableValue: 0)),
        // The bar count, not the maximum, is the scale — four bars only when the phone
        // allows them; a maximum of 4 without that draws on five bars.
        (.bars(level: 4, max: 4, operator: nil, fourBarGlyphs: false), DeviceStatusLine(.cellular, "Mobile Network", "cellularbars", variableValue: 0.8)),
        (.bars(level: 4, max: 4, operator: nil, fourBarGlyphs: true), DeviceStatusLine(.cellular, "Mobile Network", "cellularbars", variableValue: 1)),
        (.bars(level: 5, max: 4, operator: nil, fourBarGlyphs: true), DeviceStatusLine(.cellular, "Mobile Network", "cellularbars", variableValue: 0)),
        (.bars(level: 6, max: 5, operator: nil, fourBarGlyphs: false), DeviceStatusLine(.cellular, "Mobile Network", "cellularbars", variableValue: 0)),
    ])
    func cellularLines(cellular: CellularState, line: DeviceStatusLine) {
        start(makeStatus(battery: .unknown, cellular: cellular, wifi: .unknown, bluetooth: .unknown))
        #expect(model.lines == [line])
    }

    @Test func nothingIsShownOrClickableWhileThePhoneIsAway() {
        start(makeStatus(live: false))
        #expect(model.lines.isEmpty)
        #expect(model.doNotDisturb?.isEnabled == false)
        #expect(model.ringer?.isEnabled == false)
        #expect(model.ringer?.hint == nil)
    }

    @Test(arguments: [
        (RingerDisplay.mode(mode: 2), "Ringer: Sound", "Click to switch to Vibrate" as String?, "bell", true),
        (.mode(mode: 1), "Ringer: Vibrate", "Click to switch to Silent", "iphone.radiowaves.left.and.right", true),
        (.mode(mode: 0), "Ringer: Silent", "Click to switch to Sound", "bell.slash", true),
        (.turningVibrate, "Ringer: Switching to Vibrate…", nil, "iphone.radiowaves.left.and.right", false),
        (.turningOff, "Ringer: Switching to Silent…", nil, "bell.slash", false),
        (.turningOn, "Ringer: Switching to Sound…", nil, "bell", false),
        (.unknown, "Ringer", nil, "bell", false),
        // Another mode the phone reported stays clickable; the command sends Sound.
        (.mode(mode: 7), "Ringer", "Click to switch to Sound", "bell", true),
        (.mode(mode: 3), "Ringer", "Click to switch to Sound", "bell", true),
    ])
    func theRingerNamesItsModeAndTheNextOne(shown: RingerDisplay, title: String, hint: String?, symbol: String, enabled: Bool) throws {
        start(makeStatus(ringer: RingerControlState(shown: shown, enabled: true)))
        let ringer = try #require(model.ringer)
        #expect(ringer.title == title)
        #expect(ringer.hint == hint)
        #expect(ringer.symbol == symbol)
        #expect(ringer.isEnabled == enabled)
    }

    @Test func aRingerTheEngineDisabledHasNoHint() throws {
        start(makeStatus(ringer: RingerControlState(shown: .mode(mode: 2), enabled: false)))
        let ringer = try #require(model.ringer)
        #expect(ringer.title == "Ringer: Sound")
        #expect(ringer.hint == nil)
        #expect(!ringer.isEnabled)
    }

    @Test func doNotDisturbIsAToggleUnlessAChangeIsPending() throws {
        start(makeStatus(dnd: DndControlState(shown: .status(status: 2, on: true), enabled: true)))
        let on = try #require(model.doNotDisturb)
        #expect(on.title == "Do Not Disturb")
        #expect(on.isOn == true)
        #expect(on.isEnabled)

        api.update { $0.status = .success(makeStatus(dnd: DndControlState(shown: .turningOff, enabled: false))) }
        hub.post(.deviceStatus(change: .changed))
        let pending = try #require(model.doNotDisturb)
        #expect(pending.title == "Turning Off Do Not Disturb…")
        #expect(pending.isOn == nil)
        #expect(!pending.isEnabled)

        api.update { $0.status = .success(makeStatus(dnd: nil, ringer: nil)) }
        hub.post(.deviceStatus(change: .changed))
        #expect(model.doNotDisturb == nil)
        #expect(model.ringer == nil)
    }

    // MARK: - Prompts

    @Test func aPromptAControlReturnsIsShownAndItsActionRunsWhenAccepted() async {
        start()
        api.update { $0.toggle = .success(.prompt(prompt: .dndNotificationsPermission)) }
        model.toggleDoNotDisturb()
        await model.presentation?.value
        #expect(alerts.shown == [.prompt(.dndNotificationsPermission)])
        #expect(api.accepted == [.dndNotificationsPermission])
    }

    @Test func aDeclinedPromptRunsNothing() async {
        start()
        alerts.answer = false
        api.update { $0.cycle = .success(.prompt(prompt: .ringerNotificationsPermission)) }
        model.cycleRingerMode()
        await model.presentation?.value
        #expect(alerts.shown == [.prompt(.ringerNotificationsPermission)])
        #expect(api.accepted.isEmpty)
    }

    @Test func aSentCommandShowsNoAlertAndRereadsTheStatus() {
        start()
        api.update {
            $0.cycle = .success(.sent)
            $0.status = .success(makeStatus(ringer: RingerControlState(shown: .turningVibrate, enabled: false)))
        }
        model.cycleRingerMode()
        #expect(model.presentation == nil)
        #expect(model.ringer?.title == "Ringer: Switching to Vibrate…")
    }

    @Test func alertsComeOneAtATimeAndAWithdrawnPromptIsDropped() async {
        start()
        alerts.holds = true
        hub.post(.deviceStatus(change: .promptRequested(prompt: .dndTurnOffFailed)))
        await waitUntil { alerts.isShowing }

        // While it shows: the same prompt again is not queued; another one is, until withdrawn.
        hub.post(.deviceStatus(change: .promptRequested(prompt: .dndTurnOffFailed)))
        api.update { $0.toggle = .success(.prompt(prompt: .dndRuleDisabled)) }
        model.toggleDoNotDisturb()
        hub.post(.deviceStatus(change: .promptRequested(prompt: .ringerNotificationsPermission)))
        hub.post(.deviceStatus(change: .promptWithdrawn(prompt: .dndRuleDisabled)))
        alerts.release(true)

        await waitUntil { alerts.isShowing }
        alerts.release(false)
        await model.presentation?.value
        #expect(alerts.shown == [.prompt(.dndTurnOffFailed), .prompt(.ringerNotificationsPermission)])
        #expect(api.accepted == [.dndTurnOffFailed])
    }

    @Test func aControlThatFailsIsReportedButNotBeforeTheEngineStarts() async {
        start()
        api.update { $0.toggle = .failure(.NotStarted) }
        model.toggleDoNotDisturb()
        #expect(model.presentation == nil)

        api.update { $0.toggle = .failure(.Internal(message: "boom")) }
        model.toggleDoNotDisturb()
        await model.presentation?.value

        let failure = DeviceStatusAlert.failure(
            title: "Couldn't Change Do Not Disturb",
            message: UserText.describe(FeatureApiError.Internal(message: "boom"))
        )
        #expect(alerts.shown == [failure])
        #expect(!failure.isPrompt)
        #expect(failure.actionTitle == "OK")
    }

    @Test func anAcceptedPromptThatFailsIsReported() async {
        start()
        api.update {
            $0.toggle = .success(.prompt(prompt: .dndRuleDisabled))
            $0.acceptError = .NoPhone
        }
        model.toggleDoNotDisturb()
        await model.presentation?.value
        #expect(alerts.shown == [
            .prompt(.dndRuleDisabled),
            .failure(title: "Couldn't Send Request to Phone", message: UserText.describe(FeatureApiError.NoPhone)),
        ])
    }

    @Test(arguments: [
        (StatusPrompt.dndNotificationsPermission, "Allow on Phone"),
        (.ringerNotificationsPermission, "Allow on Phone"),
        (.dndRuleDisabled, "Open Settings on Phone"),
        (.dndTurnOffFailed, "Open Settings on Phone"),
        (.stopRinging, "Stop Ringing"),
    ])
    func eachPromptOffersItsAction(prompt: StatusPrompt, action: String) {
        let alert = DeviceStatusAlert.prompt(prompt)
        #expect(alert.actionTitle == action)
        #expect(alert.isPrompt)
        #expect(!alert.title.isEmpty && !alert.message.isEmpty)
    }

    // MARK: - Ring my phone

    @Test func theRingItemsFollowTheControl() throws {
        start()
        #expect(model.ringMyPhone == nil, "no control: no items")

        start(makeStatus(ring: RingControlState(shown: .off, enabled: true, canStop: false)))
        let off = try #require(model.ringMyPhone)
        #expect(off.title == "Ring Phone")
        #expect(!off.isRinging && off.isEnabled && off.stopTitle == nil)
        #expect(off.symbol == "bell.and.waves.left.and.right")

        let ringingState = RingControlState(shown: .ringing(remainingSeconds: 17, confirmStop: false), enabled: true, canStop: true)
        start(makeStatus(ring: ringingState))
        let ringing = try #require(model.ringMyPhone)
        #expect(ringing.title == "Ringing… 17 s")
        #expect(ringing.isRinging && !ringing.isEnabled)
        #expect(ringing.stopTitle == "Stop Ringing")

        start(makeStatus(ring: RingControlState(shown: .ringing(remainingSeconds: 3, confirmStop: true), enabled: true, canStop: false)))
        #expect(model.ringMyPhone?.stopTitle == nil, "the engine offers no stop")

        start(makeStatus(live: false, ring: RingControlState(shown: .unknown, enabled: false, canStop: false)))
        let away = try #require(model.ringMyPhone)
        #expect(away.title == "Ring Phone")
        #expect(!away.isEnabled)
    }

    @Test func ringingAndStoppingCallTheEngine() async {
        start(makeStatus(ring: RingControlState(shown: .off, enabled: true, canStop: false)))
        let ringingState = RingControlState(shown: .ringing(remainingSeconds: 20, confirmStop: false), enabled: true, canStop: true)
        api.update { $0.status = .success(makeStatus(ring: ringingState)) }

        model.ringPhone()
        #expect(api.rings == 1)
        #expect(model.presentation == nil)
        #expect(model.ringMyPhone?.title == "Ringing… 20 s", "re-read at once")

        // The first stop asks; accepting stops.
        api.update { $0.stop = .success(.prompt(prompt: .stopRinging)) }
        model.stopRinging()
        await model.presentation?.value
        #expect(api.stops == 1)
        #expect(alerts.shown == [.prompt(.stopRinging)])
        #expect(api.accepted == [.stopRinging])
    }

    @Test func clickingTheLowBatteryNotificationOpensSettings() {
        start()
        hub.post(.deviceStatus(change: .lowBatteryNotificationActivated))
        #expect(settings.count == 1)
    }

    // MARK: - The phone area

    @Test func theSummaryIsTheStatusBarAndNothingWhileTheStatusIsUnknown() {
        hub.postStarted()
        #expect(model.summary == nil)

        start(makeStatus(bluetooth: .unknown))
        #expect(model.summary == PhoneStatusSummary(
            wifi: .init(symbol: "wifi", variableValue: 1, label: "Wi-Fi"),
            cellular: .init(symbol: "cellularbars", variableValue: 0.75, label: "SKT"),
            batteryPercent: 78,
            battery: .init(symbol: "battery.100percent.bolt", variableValue: nil, label: "78% · Charging")
        ))

        start(makeStatus(live: false))
        #expect(model.summary == nil)
    }

    @Test func tilesNameTheControlAndItsStateApart() {
        start(makeStatus(
            dnd: DndControlState(shown: .status(status: 1, on: true), enabled: true),
            ringer: RingerControlState(shown: .mode(mode: 1), enabled: true),
            ring: RingControlState(shown: .off, enabled: true, canStop: true)
        ))
        #expect(model.doNotDisturbTile == PhoneControlTile(.doNotDisturb, "Do Not Disturb", state: "On", symbol: "moon.fill", isOn: true))
        #expect(model.ringerTile == PhoneControlTile(.ringer, "Ringer", state: "Vibrate", symbol: "iphone.radiowaves.left.and.right"))
        #expect(model.findPhoneTile == PhoneControlTile(.findPhone, "Find Phone", symbol: "bell.and.waves.left.and.right"))

        start(makeStatus(
            dnd: DndControlState(shown: .turningOff, enabled: true),
            ringer: RingerControlState(shown: .turningOff, enabled: true),
            ring: RingControlState(shown: .ringing(remainingSeconds: 23, confirmStop: false), enabled: true, canStop: true)
        ))
        #expect(model.doNotDisturbTile == PhoneControlTile(.doNotDisturb, "Do Not Disturb", state: "Turning Off…", symbol: "moon.fill", isBusy: true, isEnabled: false))
        #expect(model.ringerTile == PhoneControlTile(.ringer, "Ringer", state: "Switching…", symbol: "bell.slash", isBusy: true, isEnabled: false))
        #expect(model.findPhoneTile == PhoneControlTile(.findPhone, "Find Phone", state: "Ringing · 23 s", symbol: "bell.and.waves.left.and.right.fill", isOn: true))
    }

    @Test func tilesShowNoStateWhileThePhoneIsNotLive() {
        start(makeStatus(
            live: false,
            dnd: DndControlState(shown: .status(status: 1, on: true), enabled: true),
            ring: RingControlState(shown: .off, enabled: true, canStop: true)
        ))
        #expect(model.doNotDisturbTile == PhoneControlTile(.doNotDisturb, "Do Not Disturb", symbol: "moon.fill", isEnabled: false))
        #expect(model.ringerTile == PhoneControlTile(.ringer, "Ringer", symbol: "bell", isEnabled: false))
        #expect(model.findPhoneTile == PhoneControlTile(.findPhone, "Find Phone", symbol: "bell.and.waves.left.and.right", isEnabled: false))
    }

    @Test func theFindPhoneTileRingsOrStops() async {
        start(makeStatus(ring: RingControlState(shown: .off, enabled: true, canStop: true)))
        model.findPhone()
        #expect(api.rings == 1)

        start(makeStatus(ring: RingControlState(shown: .ringing(remainingSeconds: 20, confirmStop: false), enabled: true, canStop: true)))
        model.findPhone()
        await model.presentation?.value
        #expect(api.stops == 1)
    }
}

// MARK: - Fixtures

private func makeStatus(
    live: Bool = true,
    battery: BatteryState = .level(percent: 78, charging: true, powerSave: false),
    cellular: CellularState = .bars(level: 3, max: 4, operator: "SKT", fourBarGlyphs: true),
    wifi: WifiState = .bars(level: 4),
    bluetooth: RadioStatus = .on,
    mobileData: Bool? = nil,
    metered: Bool = false,
    dnd: DndControlState? = DndControlState(shown: .status(status: 1, on: false), enabled: true),
    ringer: RingerControlState? = RingerControlState(shown: .mode(mode: 2), enabled: true),
    ring: RingControlState? = nil
) -> PhoneStatus {
    PhoneStatus(
        live: live,
        battery: battery,
        cellular: cellular,
        wifi: wifi,
        bluetooth: bluetooth,
        mobileData: mobileData,
        metered: metered,
        doNotDisturb: dnd,
        ringer: ringer,
        ringMyPhone: ring
    )
}

@MainActor
private func waitUntil(_ condition: () -> Bool) async {
    for _ in 0..<1000 {
        if condition() { return }
        try? await Task.sleep(for: .milliseconds(2))
    }
}

@MainActor
private final class SettingsOpenCounter {
    var count = 0
}

/// Stands in for the alert panel: records what it was asked to show and answers for the user, at once
/// or, while `holds`, when the test releases it.
@MainActor
private final class AlertSpy {
    var shown: [DeviceStatusAlert] = []
    var answer = true
    var holds = false
    private var waiting: CheckedContinuation<Bool, Never>?

    var isShowing: Bool {
        waiting != nil
    }

    func present(_ alert: DeviceStatusAlert) async -> Bool {
        shown.append(alert)
        guard holds else { return answer }
        return await withCheckedContinuation { waiting = $0 }
    }

    func release(_ answer: Bool) {
        guard let waiting else { preconditionFailure("no alert is showing") }
        self.waiting = nil
        waiting.resume(returning: answer)
    }
}

/// The device-status API with canned answers.
private final class DeviceStatusApiFake: DeviceStatusApiProtocol, @unchecked Sendable {
    struct Store {
        var status: Result<PhoneStatus, FeatureApiError> = .failure(.NotStarted)
        var toggle: Result<StatusControlOutcome, FeatureApiError> = .success(.sent)
        var cycle: Result<StatusControlOutcome, FeatureApiError> = .success(.sent)
        var ring: Result<StatusControlOutcome, FeatureApiError> = .success(.sent)
        var stop: Result<StatusControlOutcome, FeatureApiError> = .success(.sent)
        var rings = 0
        var stops = 0
        var acceptError: FeatureApiError?
        var accepted: [StatusPrompt] = []
    }

    private let lock = NSLock()
    private var store = Store()

    func update(_ change: (inout Store) -> Void) {
        lock.withLock { change(&store) }
    }

    var accepted: [StatusPrompt] {
        lock.withLock { store.accepted }
    }

    func status() throws -> PhoneStatus {
        try lock.withLock { store.status }.get()
    }

    func toggleDoNotDisturb() throws -> StatusControlOutcome {
        try lock.withLock { store.toggle }.get()
    }

    func cycleRingerMode() throws -> StatusControlOutcome {
        try lock.withLock { store.cycle }.get()
    }

    var rings: Int {
        lock.withLock { store.rings }
    }

    var stops: Int {
        lock.withLock { store.stops }
    }

    func ringMyPhone() throws -> StatusControlOutcome {
        try lock.withLock {
            store.rings += 1
            return store.ring
        }.get()
    }

    func stopRinging() throws -> StatusControlOutcome {
        try lock.withLock {
            store.stops += 1
            return store.stop
        }.get()
    }

    func acceptPrompt(prompt: StatusPrompt) throws {
        let error: FeatureApiError? = lock.withLock {
            store.accepted.append(prompt)
            return store.acceptError
        }
        if let error { throw error }
    }

    // The low-battery setting is the Settings part's.

    func lowBatteryNotificationsEnabled() throws -> Bool {
        preconditionFailure("not used by DeviceStatusModel")
    }

    func setLowBatteryNotificationsEnabled(enabled: Bool) throws {
        preconditionFailure("not used by DeviceStatusModel")
    }
}
