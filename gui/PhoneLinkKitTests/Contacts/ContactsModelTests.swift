import AppKit
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

private struct ContactsData {
    var state = contactsState()
    var contacts: [ContactListItem] = []
    var cards: [Int64: ContactCard] = [:]
    var permission = ContactsPermissionResult(outcome: .success, timedOut: false, sync: .success)
    var consents: [Bool] = []
    var permissionRequests = 0
    var listReads = 0
}

private final class FakeContacts: ContactsApiProtocol, @unchecked Sendable {
    let data = FakeBox(ContactsData())

    func state() throws -> ContactsState {
        data.with { $0.state }
    }

    func list() throws -> [ContactListItem] {
        data.with { d in
            d.listReads += 1
            return d.state.access == .granted ? d.contacts : []
        }
    }

    func card(contactId: Int64) throws -> ContactCard? {
        data.with { $0.state.access == .granted ? $0.cards[contactId] : nil }
    }

    func suggestions(query: String, limit: UInt32) throws -> [ContactSuggestion] { [] }

    func thumbnail(contactId: Int64) throws -> Data? { nil }

    func setPcConsent(consent: Bool) throws {
        data.with { d in
            d.consents.append(consent)
            d.state.pcConsent = consent
            d.state.access = consent ? .granted : .pcConsentRequired
        }
    }

    func requestPermission() async throws -> ContactsPermissionResult {
        data.with { d in
            d.permissionRequests += 1
            if d.permission.outcome == .success && !d.permission.timedOut {
                d.state.access = .granted
            }
            return d.permission
        }
    }
}

// MARK: - Fixtures

private func contactsState(access: ContactsAccess = .granted, upToDate: Bool = true, capable: Bool = true) -> ContactsState {
    ContactsState(capable: capable, pcConsent: access != .pcConsentRequired, access: access, upToDate: upToDate, lastUpdatedMs: nil)
}

private func item(_ id: Int64, _ name: String, _ terms: [String] = []) -> ContactListItem {
    ContactListItem(contactId: id, name: name, hasThumbnail: false, searchTerms: terms)
}

private func card(_ id: Int64, _ name: String, numbers: [String] = []) -> ContactCard {
    ContactCard(
        contactId: id, name: name, nickname: "", company: "", jobTitle: "", notes: "", hasThumbnail: false,
        phoneNumbers: numbers.map { ContactCardNumber(number: $0, shown: $0, label: .mobile) },
        emailAddresses: [], postalAddresses: [], urls: [], dates: []
    )
}

@MainActor
private final class World {
    let api = FakeContacts()
    let hub = EngineEventHub()
    let navigator = AppNavigator()
    var opened: [URL] = []
    var model: ContactsModel!

    init(
        state: ContactsState = contactsState(),
        contacts: [ContactListItem] = [],
        timing: ContactsTiming = ContactsTiming(searchDelay: .zero, syncWait: .seconds(120))
    ) {
        api.data.with {
            $0.state = state
            $0.contacts = contacts
        }

        model = ContactsModel(
            api: api,
            hub: hub,
            navigator: navigator,
            reads: InlineReads(),
            timing: timing,
            openURL: { [unowned self] url in opened.append(url) }
        )

        hub.postStarted()
    }

    func titles() -> [String] {
        model.sections.map(\.title)
    }

    func names() -> [[String]] {
        model.sections.map { $0.contacts.map(\.name) }
    }
}

/// Waits (briefly) for something an async call does.
@MainActor
private func eventually(_ condition: () -> Bool) async {
    for _ in 0..<1000 {
        if condition() { return }
        try? await Task.sleep(for: .milliseconds(2))
    }
    Issue.record("the condition never held")
}

// MARK: - The index

@Test(arguments: [
    ("Alex", "A"),
    ("élodie", "E"),
    ("  zoe", "Z"),
    ("김민지", "ㄱ"),
    ("꽃집", "ㄱ"),
    ("하늘", "ㅎ"),
    ("ㅆ", "ㅅ"),
    ("1577-7011", "#"),
    ("山田", "#"),
    ("", "#"),
])
func aNameGoesUnderTheGroupOfItsFirstCharacter(name: String, group: String) {
    #expect(ContactsIndex.group(of: name) == group)
}

@Test func groupsRunLatinThenHangulThenOtherWithNamesInFinderOrder() {
    let sections = ContactsIndex.sections([
        item(1, "박서준"), item(7, "Bo"), item(3, ""), item(4, "Amy 10"), item(5, "Amy 9"), item(6, "강하늘"), item(2, "Bo"),
    ])
    #expect(sections.map(\.title) == ["A", "B", "ㄱ", "ㅂ", "#"])
    #expect(sections[0].contacts.map(\.contactId) == [5, 4], "numbers in names compare by value")
    #expect(sections[1].contacts.map(\.contactId) == [2, 7], "names that compare the same go by id")
}

@Test(arguments: [
    ("amy", true),
    ("AMY KIM", true),
    ("kim amy", true),
    ("emile", true),
    ("contoso", true),
    ("0101234", true),
    ("1234-5678", true),
    ("amy zed", false),
    ("99999", false),
])
func everyTokenOfTheSearchIsFoundInSomeField(query: String, found: Bool) {
    let contact = item(1, "Amy Kim", ["Émile", "Contoso", "010-1234-5678"])
    #expect(ContactsIndex.matches(contact, query: query) == found)
}

// MARK: - The model

@MainActor
@Test func startListsTheContactsUnderTheirGroups() {
    let world = World(contacts: [item(1, "Kim"), item(2, "Amy"), item(3, "")])
    #expect(world.model.isViewVisible)
    #expect(world.model.content == .list)
    #expect(world.titles() == ["A", "K", "#"])
}

@MainActor
@Test func thePromptStandsInForTheListWhileContactsMayNotBeUsed() {
    let off = World(state: contactsState(access: .pcConsentRequired), contacts: [item(1, "Kim")])
    #expect(off.model.content == .turnedOff)
    off.model.turnOn()
    #expect(off.api.data.with { $0.consents } == [true])
    #expect(off.model.content == .list, "turning contacts on reads the list")

    let notAllowed = World(state: contactsState(access: .deviceConsentRequired))
    #expect(notAllowed.model.content == .needsPhoneAccess(denied: false))
    let denied = World(state: contactsState(access: .deviceConsentDenied))
    #expect(denied.model.content == .needsPhoneAccess(denied: true))
}

@MainActor
@Test func aPhoneThatDoesNotSyncContactsHasNoContactsSection() {
    let world = World(state: contactsState(capable: false))
    #expect(!world.model.isViewVisible)
}

@MainActor
@Test func allowingContactsOnThePhoneShowsTheList() async {
    let world = World(state: contactsState(access: .deviceConsentRequired), contacts: [item(1, "Kim")])
    await world.model.requestPermission()
    #expect(world.api.data.with { $0.permissionRequests } == 1)
    #expect(world.model.content == .list)
    #expect(world.model.alert == nil)
}

@MainActor
@Test func aPhoneThatDoesNotAnswerLeavesThePromptWithItsNotice() async {
    let world = World(state: contactsState(access: .deviceConsentRequired))
    world.api.data.with { $0.permission = ContactsPermissionResult(outcome: .timedOut, timedOut: true, sync: nil) }
    await world.model.requestPermission()
    #expect(world.model.permissionTimedOut)
    #expect(world.model.content == .needsPhoneAccess(denied: false))
    #expect(world.model.alert == nil, "the prompt says it; no alert")
}

@MainActor
@Test func aRefusedPermissionIsAnAlert() async {
    let world = World(state: contactsState(access: .deviceConsentRequired))
    world.api.data.with { $0.permission = ContactsPermissionResult(outcome: .denied, timedOut: false, sync: nil) }
    await world.model.requestPermission()
    #expect(world.model.alert != nil)
}

@MainActor
@Test func contactsThatAreNotUpToDateShowAsSyncingUntilTheWaitRunsOut() async {
    let world = World(
        state: contactsState(upToDate: false),
        timing: ContactsTiming(searchDelay: .zero, syncWait: .milliseconds(20))
    )
    #expect(world.model.content == .syncing)
    await eventually { world.model.content != .syncing }
    #expect(world.model.content == .empty)
}

@MainActor
@Test func theSearchFiltersOnceTypingPauses() async {
    let world = World(
        contacts: [item(1, "Amy", ["010-1234-5678"]), item(2, "Bo")],
        timing: ContactsTiming(searchDelay: .milliseconds(10), syncWait: .seconds(120))
    )

    world.model.searchText = "1234"
    #expect(world.names() == [["Amy"], ["Bo"]], "not yet")
    await eventually { world.names() == [["Amy"]] }
    #expect(world.names() == [["Amy"]])

    world.model.searchText = "zzz"
    await eventually { world.model.searchFoundNothing }
    #expect(world.model.searchFoundNothing)

    world.model.searchText = ""
    #expect(world.names() == [["Amy"], ["Bo"]], "clearing the field shows the list at once")
}

@MainActor
@Test func selectingAContactReadsItsCardAndAGoneContactIsDeselected() {
    let world = World(contacts: [item(1, "Kim")])
    world.api.data.with { $0.cards[1] = card(1, "Kim") }
    world.model.selection = 1
    #expect(world.model.card?.name == "Kim")

    world.api.data.with {
        $0.contacts = []
        $0.cards = [:]
    }

    world.hub.post(.contacts(change: .recordsChanged))
    #expect(world.model.selection == nil)
    #expect(world.model.card == nil)
}

@MainActor
@Test func aNumbersMessageButtonOpensMessagesAddressedToIt() {
    let world = World(contacts: [item(1, "Kim")])
    world.api.data.with { $0.cards[1] = card(1, "Kim", numbers: ["+821012345678"]) }
    world.model.selection = 1

    world.model.message(world.model.card!.phoneNumbers[0])
    #expect(world.navigator.section == .messages)
    #expect(world.navigator.messageRequest == MessagesRecipient(address: "+821012345678", name: "Kim"))
}

@MainActor
@Test func fieldsOpenInTheirApps() {
    let world = World()
    world.model.email("a.b+c@example.com")
    world.model.showOnMap("1 Main St, Seattle")
    world.model.open(link: "contoso.com/about")
    world.model.open(link: "https://example.com/x")

    #expect(world.opened.map(\.absoluteString) == [
        "mailto:a.b%2Bc%40example.com",
        "maps://?q=1%20Main%20St%2C%20Seattle",
        "http://contoso.com/about",
        "https://example.com/x",
    ])
}

// MARK: - The card's texts

@Test func labelsComeFromTheFieldsTypeAndTheGenericOneFromItsKind() {
    #expect(ContactsFormat.label(.workMobile, in: .phone) == "work mobile")
    #expect(ContactsFormat.label(.custom(label: "Boat"), in: .phone) == "Boat")
    #expect(ContactsFormat.label(.generic, in: .phone) == "phone")
    #expect(ContactsFormat.label(.generic, in: .url) == "homepage")
    #expect(ContactsFormat.label(.generic, in: .date) == "date")
}

@Test func theSubtitleIsTheJobAndTheCompanyUnlessTheCompanyIsTheName() {
    var c = card(1, "Contoso")
    c.company = "Contoso"
    #expect(ContactsFormat.subtitle(c) == nil)

    c.jobTitle = "Engineer"
    c.name = "Amy"
    #expect(ContactsFormat.subtitle(c) == "Engineer · Contoso")
}
