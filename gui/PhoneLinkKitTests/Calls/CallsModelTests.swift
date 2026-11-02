import Foundation
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

// MARK: - Fakes

/// A value shared between the test and the fake's async methods, which run off the main actor.
private final class FakeBox<Value>: @unchecked Sendable {
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

/// Reads in place: the model's state follows each call at once.
@MainActor
private final class InlineReads: MessagesReading {
    func read<T: Sendable>(
        _ work: @escaping @Sendable () throws -> T,
        then apply: @escaping @MainActor (Result<T, any Error>) -> Void
    ) {
        apply(Result { try work() })
    }
}

private struct CallingData {
    var state = callingState()
    var calls: [CallHistoryItem] = []
    var permission = CallsPermissionResult(outcome: .accepted, timedOut: false, refresh: .success)
    var consents: [Bool] = []
    var refreshes = 0
    var seen = 0
}

private final class FakeCalling: CallingApiProtocol, @unchecked Sendable {
    let data = FakeBox(CallingData())

    func state() throws -> CallingState {
        data.with { $0.state }
    }

    func history() throws -> [CallHistoryItem] {
        data.with { $0.state.access == .granted ? $0.calls : [] }
    }

    func markSeen() throws {
        data.with { d in
            d.seen += 1
            d.state.unseenMissed = 0
        }
    }

    func setPcConsent(consent: Bool) throws {
        data.with { d in
            d.consents.append(consent)
            d.state.pcConsent = consent
            d.state.access = consent ? .devicePermissionRequired : .pcConsentRequired
        }
    }

    func refreshIfEmpty() async throws -> RequestOutcome? {
        data.with { d in
            d.refreshes += 1
            return d.refreshes == 1 ? .success : nil
        }
    }

    func requestPermission() async throws -> CallsPermissionResult {
        data.with { d in
            if d.permission.outcome == .accepted && !d.permission.timedOut {
                d.state.access = .granted
            }
            return d.permission
        }
    }
}

// MARK: - Fixtures

private func callingState(access: CallsAccess = .granted, upToDate: Bool = true, unseenMissed: UInt32 = 0) -> CallingState {
    CallingState(
        access: access, pcConsent: access != .pcConsentRequired, upToDate: upToDate, lastUpdatedMs: nil,
        refreshing: false, unseenMissed: unseenMissed
    )
}

/// 2026-11-02 15:00 UTC, a Monday.
private let noon = Date(timeIntervalSince1970: 1_793_631_600)

private func utcCalendar() -> Calendar {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = TimeZone(identifier: "UTC") ?? .gmt
    calendar.locale = Locale(identifier: "en_US")
    return calendar
}

private func call(_ id: Int64, _ number: String, _ kind: CallKind, daysAgo: Double, name: String? = nil, contactId: Int64? = nil) -> CallHistoryItem {
    CallHistoryItem(
        callId: id, number: number, name: name, contactId: contactId, photoContactId: nil, kind: kind,
        startMs: Int64((noon.timeIntervalSince1970 - daysAgo * 86_400) * 1000), durationSecs: 65, isRead: false,
        phoneAccountId: ""
    )
}

@MainActor
private final class World {
    let api = FakeCalling()
    let hub = EngineEventHub()
    let navigator = AppNavigator()
    var model: CallsModel!

    init(state: CallingState = callingState(), calls: [CallHistoryItem] = [], syncWait: Duration = .seconds(120)) {
        api.data.with {
            $0.state = state
            $0.calls = calls
        }

        model = CallsModel(
            api: api,
            hub: hub,
            navigator: navigator,
            reads: InlineReads(),
            timing: CallsTiming(syncWait: syncWait),
            format: CallsFormat(now: { noon }, calendar: utcCalendar())
        )
        hub.postStarted()
    }
}

/// Waits for the model's unstructured tasks to run.
@MainActor
private func settle(until done: () -> Bool) async {
    for _ in 0..<100 where !done() {
        await Task.yield()
    }
}

// MARK: - Tests

@MainActor
struct CallsModelTests {
    @Test func theAccessDecidesWhatStandsInForTheHistory() {
        let cases: [(CallsAccess, CallsContent, Bool)] = [
            (.unsupported, .nothing, false),
            (.pcConsentRequired, .turnedOff, true),
            (.callLogsUnsupported, .updateRequired, true),
            (.postNotificationRequired, .needsNotifications, true),
            (.devicePermissionRequired, .needsPhoneAccess, true),
        ]
        for (access, content, visible) in cases {
            let world = World(state: callingState(access: access))
            #expect(world.model.content == content, "\(access)")
            #expect(world.model.isViewVisible == visible, "\(access)")
        }
    }

    @Test func callsAreListedUnderTheirDays() {
        let world = World(calls: [
            call(1, "+1", .incoming, daysAgo: 0.1),
            call(2, "+2", .missed, daysAgo: 1),
            call(3, "+3", .outgoing, daysAgo: 3),
            call(4, "+4", .rejected, daysAgo: 12),
            call(5, "+5", .incoming, daysAgo: 45),
        ])

        #expect(world.model.content == .list)
        #expect(world.model.sections.map(\.bucket) == [.today, .yesterday, .thisWeek, .lastThirtyDays, .older])
        #expect(world.model.sections.map { $0.calls.map(\.callId) } == [[1], [2], [3], [4], [5]])
    }

    @Test func anEmptyHistoryAsksThePhoneOnceAndShowsItIsUpdating() async {
        let world = World(state: callingState(upToDate: false))

        #expect(world.model.content == .updating)
        await settle { world.api.data.with { $0.refreshes } > 0 }
        #expect(world.api.data.with { $0.refreshes } == 1)
    }

    @Test func theUpdatingWaitStartsOverWhenCallsFallBehindAgain() async throws {
        let world = World(state: callingState(upToDate: true), syncWait: .milliseconds(50))
        world.api.data.with { $0.state.upToDate = false }
        world.hub.post(.calling(change: .stateChanged))
        #expect(world.model.isUpdating)

        try await Task.sleep(for: .milliseconds(200))
        #expect(!world.model.isUpdating, "the wait did not run out")

        world.api.data.with { $0.state.upToDate = true }
        world.hub.post(.calling(change: .stateChanged))
        world.api.data.with { $0.state.upToDate = false }
        world.hub.post(.calling(change: .stateChanged))
        #expect(world.model.isUpdating, "the wait did not start over")
    }

    @Test func turningOnThenAllowingOnThePhoneShowsTheHistory() async {
        let world = World(state: callingState(access: .pcConsentRequired), calls: [call(1, "+1", .incoming, daysAgo: 0)])

        world.model.turnOn()
        #expect(world.api.data.with { $0.consents } == [true])
        #expect(world.model.content == .needsPhoneAccess)

        await world.model.requestPermission()
        #expect(world.model.content == .list)
        #expect(world.model.alert == nil)
    }

    @Test func aPermissionRequestThePhoneDidNotAnswerIsSaid() async {
        let world = World(state: callingState(access: .devicePermissionRequired))
        world.api.data.with { $0.permission = CallsPermissionResult(outcome: .timedOut, timedOut: true, refresh: nil) }

        await world.model.requestPermission()

        #expect(world.model.permissionTimedOut)
        #expect(world.model.content == .needsPhoneAccess)
    }

    @Test func showingTheSectionMarksMissedCallsSeen() {
        let world = World(state: callingState(unseenMissed: 2), calls: [call(1, "+1", .missed, daysAgo: 0)])
        #expect(world.model.badgeCount == 2)

        world.model.viewAppeared()
        world.hub.post(.calling(change: .stateChanged))

        #expect(world.api.data.with { $0.seen } == 1)
        #expect(world.model.badgeCount == 0)
    }

    @Test func aCallLeadsToItsConversationAndItsContact() {
        let world = World(calls: [call(1, "+15550100", .incoming, daysAgo: 0, name: "Ada", contactId: 7)])
        guard let ada = world.model.calls.first else {
            Issue.record("no call listed")
            return
        }

        world.model.message(ada)
        #expect(world.navigator.messageRequest == MessagesRecipient(address: "+15550100", name: "Ada"))

        world.model.showContact(ada)
        #expect(world.navigator.contactRequest == 7)
        #expect(world.navigator.section == .contacts)
    }

    @Test func theSearchMatchesNamesAndTheNumbersDigits() {
        let world = World(calls: [
            call(1, "+15550100", .incoming, daysAgo: 0, name: "Ada Lovelace"),
            call(2, "+15550199", .missed, daysAgo: 1),
            call(3, "", .missed, daysAgo: 2),
        ])

        world.model.searchText = "ada"
        #expect(world.model.sections.flatMap(\.calls).map(\.callId) == [1])
        world.model.searchText = "555-0199"
        #expect(world.model.sections.flatMap(\.calls).map(\.callId) == [2])
        world.model.searchText = "private"
        #expect(world.model.sections.flatMap(\.calls).map(\.callId) == [3])
        world.model.searchText = "zzz"
        #expect(world.model.searchFoundNothing)
    }

    @Test func theSelectedCallBringsTheOtherCallsWithItsNumber() {
        let world = World(calls: [
            call(1, "+1", .incoming, daysAgo: 0),
            call(2, "+2", .incoming, daysAgo: 1),
            call(3, "+1", .missed, daysAgo: 2),
            call(4, "", .missed, daysAgo: 3),
        ])

        world.model.selection = 3
        #expect(world.model.callsWithSelectedNumber.map(\.callId) == [1, 3])
        world.model.selection = 4
        #expect(world.model.callsWithSelectedNumber.isEmpty)
    }
}

@MainActor
struct CallsFormatTests {
    private let format = CallsFormat(now: { noon }, calendar: utcCalendar())

    @Test func voicemailBlockedAndAnsweredElsewhereShowAsIncoming() {
        for kind in [CallKind.incoming, .voicemail, .blocked, .answeredExternally] {
            #expect(CallsFormat.style(kind).label == "Incoming", "\(kind)")
            #expect(!CallsFormat.style(kind).missed, "\(kind)")
        }
        #expect(CallsFormat.style(.missed) == CallKindStyle(label: "Missed", symbol: "phone.arrow.down.left", missed: true))
        #expect(CallsFormat.style(.rejected).label == "Declined")
        #expect(CallsFormat.style(.rejected).missed)
        #expect(CallsFormat.style(.unknown) == CallKindStyle(label: "", symbol: nil, missed: false))
    }

    @Test func aCallIsNamedByItsContactElseItsNumber() {
        #expect(CallsFormat.name(call(1, "+1", .incoming, daysAgo: 0, name: "Ada")) == "Ada")
        #expect(CallsFormat.name(call(1, "+1", .incoming, daysAgo: 0, name: "")) == "+1")
        #expect(CallsFormat.name(call(1, "", .incoming, daysAgo: 0)) == "Private Number")
    }

    @Test func rowTimesFollowTheDay() {
        let ms = { (daysAgo: Double) in Int64((noon.timeIntervalSince1970 - daysAgo * 86_400) * 1000) }
        #expect(format.rowTime(ms: ms(0)) == "3:00\u{202F}PM")
        #expect(format.rowTime(ms: ms(1)) == "3:00\u{202F}PM")
        #expect(format.rowTime(ms: ms(3)) == "Friday")
        #expect(format.rowTime(ms: ms(12)) == "10/21")
        #expect(format.rowTime(ms: ms(45)) == "9/18/2026")
        #expect(format.bucket(ms: ms(-1)) == .today, "a phone clock ahead counts as today")
    }

    @Test func durationsReadAsAClock() {
        #expect(CallsFormat.duration(seconds: 0) == nil)
        #expect(CallsFormat.duration(seconds: 65) == "1:05")
        #expect(CallsFormat.duration(seconds: 3_725) == "1:02:05")
    }
}
