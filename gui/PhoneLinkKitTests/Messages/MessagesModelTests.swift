import AppKit
import Foundation
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

// MARK: - Fakes

/// A value shared between the test and the fakes' async methods, which run off the main actor.
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

/// Holds an async fake call until the test opens it.
private actor Gate {
    private var isOpen = false
    private var waiting: [CheckedContinuation<Void, Never>] = []

    func wait() async {
        guard !isOpen else { return }
        await withCheckedContinuation { waiting.append($0) }
    }

    func open() {
        isOpen = true
        waiting.forEach { $0.resume() }
        waiting.removeAll()
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

/// Reads the test runs when and in the order it chooses.
@MainActor
private final class HeldReads: MessagesReading {
    private var held: [() -> Void] = []
    var count: Int { held.count }

    func read<T: Sendable>(
        _ work: @escaping @Sendable () throws -> T,
        then apply: @escaping @MainActor (Result<T, any Error>) -> Void
    ) {
        held.append { apply(Result { try work() }) }
    }

    func run(_ index: Int) {
        held.remove(at: index)()
    }

    func runAll() {
        while !held.isEmpty {
            held.removeFirst()()
        }
    }
}

private struct MessagingData {
    var started = true
    var state = messagingState()
    var conversations: [Conversation] = []
    var messages: [Int64: [Message]] = [:]
    var messagesError: FeatureApiError?
    var unsent: [NewConversationMessage] = []
    var limits = NewMessageLimits(attachmentsAllowed: true, groupAllowed: true, maxTextLength: nil, maxRecipients: nil, activeSubscription: -1)
    var subscriptions: [SimSubscription] = []
    var partBytes: [MessagePartRef: Data] = [:]
    var partBytesError: FeatureApiError?
    var partReads: [MessagePartRef] = []
    /// Whether each `messages` read ran on the main thread.
    var readsOnMain: [Bool] = []
    /// The engine calls of the mark-as-read triggers and the other one-way calls, in order.
    var calls: [String] = []
    var messageReads: [Int64] = []
    var conversationReads = 0
    var stateReads = 0
    var sends: [(target: MessageTarget, text: String, attachments: [MessageAttachment])] = []
    var sendGate: Gate?
    var sendResult: Result<SendResult, FeatureApiError> = .success(SendResult(sendId: 1, state: .sent, outcome: .success))
    var retryResult: Result<SendResult, FeatureApiError> = .success(SendResult(sendId: 1, state: .sending, outcome: .success))
    var retries: [Int64] = []
    var downloadGate: Gate?
    var downloadResult: Result<RequestOutcome, FeatureApiError> = .success(.success)
    var downloads: [MessagePartRef] = []
    var permissionOutcome: RequestOutcome = .success
    var flags: [String] = []
    var settings: [MessagingSettings] = []
    var simChoices: [(threadId: Int64, subscriptionId: Int64?)] = []
}

private final class FakeMessaging: MessagingApiProtocol, @unchecked Sendable {
    let data = FakeBox(MessagingData())

    private func started() throws {
        if !data.with({ $0.started }) { throw FeatureApiError.NotStarted }
    }

    private func record(_ call: String) throws {
        try started()
        data.with { $0.calls.append(call) }
    }

    func composedConversationReplaced(threadId: Int64) throws { try record("replaced \(threadId)") }
    func conversationSelected(threadId: Int64?) throws { try record("selected \(threadId.map(String.init) ?? "none")") }
    func messagesViewChanged(active: Bool) throws { try record("view \(active)") }
    func windowFocused(focused: Bool) throws { try record("focused \(focused)") }

    func conversations() throws -> [Conversation] {
        try started()
        return data.with {
            $0.conversationReads += 1
            return $0.conversations
        }
    }

    func messages(threadId: Int64) throws -> [Message] {
        try started()
        let onMain = Thread.isMainThread
        return try data.with {
            $0.messageReads.append(threadId)
            $0.readsOnMain.append(onMain)
            if let error = $0.messagesError { throw error }
            return $0.messages[threadId] ?? []
        }
    }

    func newConversationMessages() throws -> [NewConversationMessage] {
        try started()
        return data.with { $0.unsent }
    }

    func newMessageLimits() throws -> NewMessageLimits {
        try started()
        return data.with { $0.limits }
    }

    func partBytes(part: MessagePartRef) throws -> Data? {
        try started()
        return try data.with {
            $0.partReads.append(part)
            if let error = $0.partBytesError { throw error }
            return $0.partBytes[part]
        }
    }

    func state() throws -> MessagingState {
        try started()
        return data.with {
            $0.stateReads += 1
            return $0.state
        }
    }

    func subscriptions() throws -> [SimSubscription] {
        try started()
        return data.with { $0.subscriptions }
    }

    func downloadPart(part: MessagePartRef) async throws -> RequestOutcome {
        let gate = data.with { d -> Gate? in
            d.downloads.append(part)
            return d.downloadGate
        }

        if let gate { await gate.wait() }
        return try data.with { try $0.downloadResult.get() }
    }

    func requestPermission() async throws -> RequestOutcome {
        data.with { $0.permissionOutcome }
    }

    func retry(sendId: Int64) async throws -> SendResult {
        try data.with { d in
            d.retries.append(sendId)
            return try d.retryResult.get()
        }
    }

    func send(target: MessageTarget, text: String, attachments: [MessageAttachment]) async throws -> SendResult {
        let gate = data.with { d -> Gate? in
            d.sends.append((target, text, attachments))
            return d.sendGate
        }

        if let gate { await gate.wait() }
        return try data.with { try $0.sendResult.get() }
    }

    func selectSubscription(threadId: Int64, subscriptionId: Int64?) throws {
        try started()
        data.with { $0.simChoices.append((threadId, subscriptionId)) }
    }

    func setConversationFlags(threadId: Int64, muted: Bool, hidden: Bool) throws {
        try started()
        data.with { d in
            d.flags.append("\(threadId) muted:\(muted) hidden:\(hidden)")
            if let index = d.conversations.firstIndex(where: { $0.threadId == threadId }) {
                d.conversations[index].muted = muted
                d.conversations[index].hidden = hidden
            }
        }
    }

    func setSettings(settings: MessagingSettings) throws {
        try started()
        data.with { d in
            d.settings.append(settings)
            d.state.settings = settings
        }
    }
}

private struct ContactsData {
    var state = ContactsState(capable: true, pcConsent: true, access: .granted, upToDate: true, lastUpdatedMs: nil)
    var book: [ContactSuggestion] = []
    var queries: [(query: String, limit: UInt32)] = []
    var photos: [Int64: Data] = [:]
    var photoError: FeatureApiError?
    var photoReads: [Int64] = []
    var permission = ContactsPermissionResult(outcome: .success, timedOut: false, sync: nil)
    var consents: [Bool] = []
}

private final class FakeContacts: ContactsApiProtocol, @unchecked Sendable {
    let data = FakeBox(ContactsData())

    func requestPermission() async throws -> ContactsPermissionResult {
        data.with { $0.permission }
    }

    func setPcConsent(consent: Bool) throws {
        data.with { d in
            d.consents.append(consent)
            d.state.pcConsent = consent
            d.state.access = consent ? .granted : .pcConsentRequired
        }
    }

    func state() throws -> ContactsState {
        data.with { $0.state }
    }

    func suggestions(query: String, limit: UInt32) throws -> [ContactSuggestion] {
        data.with { d in
            d.queries.append((query, limit))
            let matches = d.book.filter { $0.name.localizedCaseInsensitiveContains(query) || $0.number.contains(query) }
            return Array(matches.prefix(Int(limit)))
        }
    }

    func list() throws -> [ContactListItem] { [] }

    func card(contactId: Int64) throws -> ContactCard? { nil }

    func thumbnail(contactId: Int64) throws -> Data? {
        try data.with { d in
            d.photoReads.append(contactId)
            if let error = d.photoError { throw error }
            return d.photos[contactId]
        }
    }
}

// MARK: - Fixtures

private let utcCalendar: Calendar = {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = TimeZone(identifier: "UTC")!
    calendar.locale = Locale(identifier: "en_US")
    return calendar
}()

/// Wednesday, 21 October 2026, 14:00 UTC.
private let testNow = utcCalendar.date(from: DateComponents(year: 2026, month: 10, day: 21, hour: 14))!

private func person(_ address: String, _ name: String? = nil, contactId: Int64? = nil, photo: Bool = false) -> MessageContact {
    MessageContact(address: address, name: name, contactId: contactId, photoContactId: photo ? contactId : nil)
}

private func conversation(
    _ threadId: Int64,
    _ people: [MessageContact],
    latestMs: Int64 = 0,
    preview: ConversationPreviewLine? = nil,
    unread: Int64 = 0,
    muted: Bool = false,
    hidden: Bool = false,
    canReply: Bool = true,
    attachmentsAllowed: Bool = true,
    activeSubscription: Int64 = -1,
    maxTextLength: Int32? = nil
) -> Conversation {
    Conversation(
        threadId: threadId,
        recipients: people,
        isGroup: people.count > 1,
        latestMs: latestMs,
        preview: preview,
        unread: unread,
        muted: muted,
        hidden: hidden,
        canReply: canReply,
        attachmentsAllowed: attachmentsAllowed,
        activeSubscription: activeSubscription,
        maxTextLength: maxTextLength,
        maxRecipients: nil
    )
}

private func message(
    _ id: Int64,
    at ms: Int64,
    incoming: Bool = true,
    from contact: MessageContact = person("+15550100"),
    body: String = "Hi",
    kind: MessageType = .sms,
    parts: [MessagePart] = [],
    sendState: MessageSendState? = nil
) -> Message {
    Message(
        kind: kind,
        id: id,
        timestampMs: ms,
        incoming: incoming,
        mailbox: incoming ? .inbox : .sent,
        contact: contact,
        body: body,
        subject: "",
        read: true,
        subscriptionId: -1,
        groupEvent: nil,
        parts: parts,
        sendState: sendState
    )
}

/// A message sent from this Mac to new recipients, as the engine lists it.
private func unsentMessage(_ sendId: Int64, to people: [MessageContact], at ms: Int64, body: String = "Hello", state: MessageSendState) -> NewConversationMessage {
    NewConversationMessage(
        recipients: people,
        message: message(sendId, at: ms, incoming: false, from: person(""), body: body, kind: .pendingSend, sendState: state)
    )
}

private func messagingState(
    pcConsent: Bool = true,
    permission: Bool = true,
    syncing: Bool = false,
    upToDate: Bool = true,
    sendMms: Bool = true,
    proximity: ProximityState? = nil
) -> MessagingState {
    MessagingState(
        phonePermission: permission,
        settings: MessagingSettings(receiveMms: true, autoDownloadMms: true, sendMms: sendMms, pcConsent: pcConsent, messageNotifications: true),
        upToDate: upToDate,
        syncing: syncing,
        lastUpdatedMs: nil,
        setupCompletedOnce: true,
        proximity: proximity
    )
}

private func image(_ name: String = "photo.png") -> MessagesAttachment {
    MessagesAttachment(name: name, contentType: "image/png", data: Data([0x89, 0x50, 0x4E, 0x47]))
}

/// A 2×2 PNG ImageIO decodes.
private let pngData: Data = {
    let image = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 2, pixelsHigh: 2, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    return image.representation(using: .png, properties: [:])!
}()

private func suggestion(_ contactId: Int64, _ name: String, _ number: String, photo: Bool = false) -> ContactSuggestion {
    ContactSuggestion(contactId: contactId, name: name, number: number, kind: .mobile, label: "", hasThumbnail: photo)
}

@MainActor
private final class World {
    let messaging = FakeMessaging()
    let contacts = FakeContacts()
    let hub = EngineEventHub()
    let navigator = AppNavigator()
    let model: MessagesModel

    init(
        conversations: [Conversation] = [],
        state: MessagingState = messagingState(),
        reads: any MessagesReading = InlineReads(),
        timing: MessagesTiming = MessagesTiming(searchDelay: .zero, downloadWait: .seconds(30))
    ) {
        messaging.data.with {
            $0.conversations = conversations
            $0.state = state
        }

        let now = testNow
        model = MessagesModel(
            api: messaging,
            contacts: contacts,
            hub: hub,
            navigator: navigator,
            time: MessagesTimeFormatter(now: { now }, calendar: utcCalendar),
            reads: reads,
            timing: timing
        )
    }

    /// A world whose engine has started.
    static func started(
        conversations: [Conversation] = [],
        state: MessagingState = messagingState(),
        timing: MessagesTiming = MessagesTiming(searchDelay: .zero, downloadWait: .seconds(30))
    ) -> World {
        let world = World(conversations: conversations, state: state, timing: timing)
        world.hub.postStarted()
        return world
    }

    func post(_ change: MessagingChange) {
        hub.post(.messaging(change: change))
    }

    var calls: [String] { messaging.data.with { $0.calls } }
    var messageReads: [Int64] { messaging.data.with { $0.messageReads } }
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

private let alex = person("+15550101", "Alex Kim")
private let minji = person("+821055556666", "김민지")
private let unknown = person("1588-0000")

// MARK: - Loading and events

@MainActor
@Test func startLoadsWhatTheSectionShows() {
    let world = World(conversations: [conversation(1, [alex])])
    world.messaging.data.with {
        $0.subscriptions = [SimSubscription(id: 3, simSlotIndex: 0, name: "Work", number: "", isDefaultSms: true)]
        $0.limits.maxTextLength = 160
    }
    #expect(world.model.conversations.isEmpty)
    #expect(world.model.state == nil)

    world.hub.postStarted()

    #expect(world.model.conversations.map(\.threadId) == [1])
    #expect(world.model.state == messagingState())
    #expect(world.model.subscriptions.map(\.id) == [3])
    #expect(world.model.contactsState?.access == .granted)
    #expect(world.model.newMessageLimits?.maxTextLength == 160)
}

@MainActor
@Test func beforeTheEngineStartsNothingShowsAndNothingIsAnError() {
    let world = World(conversations: [conversation(1, [alex])])
    world.messaging.data.with { $0.started = false }

    world.hub.postStarted()
    world.model.select(1)
    world.model.setWindowFocused(true)

    #expect(world.model.conversations.isEmpty)
    #expect(world.model.state == nil)
    #expect(world.model.transcript.isEmpty)
    #expect(!world.model.transcriptFailed, "no engine yet is nothing to show, not a failure")
    #expect(world.model.alert == nil)
}

@MainActor
@Test func contentChangesReloadTheShownTranscriptOnlyWhenItIsTouched() {
    let world = World.started(conversations: [conversation(1, [alex]), conversation(2, [minji])])
    world.messaging.data.with { $0.messages[1] = [message(10, at: 1_000)] }
    world.model.select(1)
    #expect(world.messageReads == [1])
    #expect(world.model.transcript.map(\.message.id) == [10])

    let conversationReads = world.messaging.data.with { $0.conversationReads }

    world.messaging.data.with { $0.messages[1] = [message(10, at: 1_000), message(11, at: 2_000)] }
    world.post(.contentChanged(threads: [2]))
    #expect(world.messageReads == [1], "another conversation changed")
    #expect(world.messaging.data.with { $0.conversationReads } == conversationReads + 1)

    world.post(.contentChanged(threads: [2, 1]))
    #expect(world.messageReads == [1, 1])
    #expect(world.model.transcript.map(\.message.id) == [10, 11])

    world.post(.contentChanged(threads: []))
    #expect(world.messageReads == [1, 1, 1], "an empty list means every conversation")

    world.post(.sendChanged(sendId: 4))
    #expect(world.messageReads == [1, 1, 1, 1])

    world.hub.post(.resync(feature: .messaging))
    #expect(world.messageReads == [1, 1, 1, 1, 1])
}

@MainActor
@Test func stateAndPermissionChangesReloadTheState() {
    let world = World.started(conversations: [conversation(1, [alex])])
    world.messaging.data.with { $0.state = messagingState(permission: false) }

    world.post(.permissionChanged(granted: false))
    #expect(world.model.state?.phonePermission == false)
    #expect(world.model.listStatus == .permissionNeeded)

    world.messaging.data.with {
        $0.state = messagingState(sendMms: false)
        $0.conversations = [conversation(1, [alex], attachmentsAllowed: false)]
    }
    world.post(.stateChanged)
    #expect(world.model.listStatus == .conversations)
    #expect(world.model.conversations.first?.attachmentsAllowed == false, "settings decide which conversations take images")
}

@MainActor
@Test func aConversationAskedForIsShown() {
    let world = World.started(conversations: [conversation(1, [alex]), conversation(5, [minji])])

    world.post(.openConversation(threadId: 5))
    #expect(world.navigator.conversationRequest == 5)
    #expect(world.navigator.section == .messages)

    world.model.startNewMessage()
    world.model.select(5)
    #expect(world.model.selection == 5)
    #expect(!world.model.isComposing)
}

@MainActor
@Test func aConversationThatDisappearsIsNoLongerShown() {
    let world = World.started(conversations: [conversation(1, [alex]), conversation(2, [minji])])
    world.model.select(2)

    world.messaging.data.with { $0.conversations = [conversation(1, [alex])] }
    world.post(.contentChanged(threads: [2]))

    #expect(world.model.selection == nil)
    #expect(world.model.transcript.isEmpty)
    #expect(world.calls.last == "selected none")
}

// MARK: - Reads off the main actor

@MainActor
@Test func readsRunOffTheMainActorAndAnOlderResultIsDropped() async {
    // The app's reads: the transcript is read on another thread and applied when it arrives.
    let background = World(conversations: [conversation(1, [alex])], reads: MessagesBackgroundReads())
    background.messaging.data.with { $0.messages[1] = [message(10, at: 1_000)] }
    background.hub.postStarted()
    await eventually { background.model.conversations.count == 1 }
    background.model.select(1)
    await eventually { background.model.transcript.count == 1 }
    #expect(background.messaging.data.with { $0.readsOnMain } == [false])

    // Two reads of the shown conversation: the newer one's result stays when the older one comes last.
    let reads = HeldReads()
    let world = World(conversations: [conversation(1, [alex]), conversation(2, [minji])], reads: reads)
    world.hub.postStarted()
    reads.runAll()

    world.messaging.data.with {
        $0.messages[1] = [message(10, at: 1_000)]
        $0.messages[2] = [message(20, at: 1_000)]
    }

    world.model.select(1)
    world.model.select(2)
    #expect(reads.count == 2)

    reads.run(1)
    #expect(world.model.transcript.map(\.message.id) == [20])
    reads.run(0)
    #expect(world.model.transcript.map(\.message.id) == [20], "conversation 1's late messages are not shown under 2")

    // An older conversation list does not replace a newer one, whenever its read ends.
    world.post(.contentChanged(threads: [5]))
    world.post(.contentChanged(threads: [3]))
    #expect(reads.count == 2)

    world.messaging.data.with { $0.conversations.append(conversation(3, [unknown])) }
    reads.run(1)
    #expect(world.model.conversations.map(\.threadId) == [1, 2, 3])

    world.messaging.data.with { $0.conversations.removeLast() }
    reads.run(0)
    #expect(world.model.conversations.map(\.threadId) == [1, 2, 3], "the older read's list is dropped")
}

@MainActor
@Test func theRecipientSearchWaitsForTypingToPause() async {
    let world = World.started(timing: MessagesTiming(searchDelay: .milliseconds(40), downloadWait: .seconds(30)))
    world.contacts.data.with { $0.book = [suggestion(7, "Alex Kim", "+1 555-0101")] }
    world.model.startNewMessage()

    world.model.setRecipientQuery("a")
    world.model.setRecipientQuery("al")
    world.model.setRecipientQuery("ale")
    #expect(world.contacts.data.with { $0.queries.isEmpty }, "not while typing")

    await eventually { !world.model.suggestions.isEmpty }
    #expect(world.contacts.data.with { $0.queries.map(\.query) } == ["ale"], "once, for what was typed last")
}

// MARK: - Sending

@MainActor
@Test func aSendClearsTheDraftAtOnceAndTheComposerStaysOpen() async {
    let world = World.started(conversations: [conversation(1, [alex])])
    let gate = Gate()
    world.messaging.data.with { $0.sendGate = gate }
    world.model.select(1)
    world.model.setDraftText("See you at 7")
    world.model.addAttachments([image()])

    let sending = Task { await world.model.send() }
    await eventually { world.messaging.data.with { $0.sends.count } == 1 }

    #expect(world.model.draft == MessagesDraft(), "the message shows in the transcript as it is sent")
    #expect(world.model.composer.isEnabled)

    world.model.setDraftText("And another thing")
    #expect(world.model.draft.text == "And another thing", "the next message can be typed meanwhile")

    await gate.open()
    await sending.value

    let sent = world.messaging.data.with { $0.sends[0] }
    #expect(sent.target == .conversation(threadId: 1))
    #expect(sent.text == "See you at 7")
    #expect(sent.attachments == [MessageAttachment(name: "photo.png", contentType: "image/png", bytes: Data([0x89, 0x50, 0x4E, 0x47]))])
    #expect(world.model.draft.text == "And another thing")
    #expect(world.model.alert == nil)
}

@MainActor
@Test func aRefusedSendGetsItsDraftBackAndSaysWhy() async {
    let world = World.started(conversations: [conversation(1, [alex])])
    world.messaging.data.with { $0.sendResult = .failure(.InvalidInput(reason: "at most 2 attachments per message")) }
    world.model.select(1)
    world.model.setDraftText("Hello")

    await world.model.send()

    #expect(world.model.draft.text == "Hello")
    #expect(world.model.alert?.title == "Message Not Sent")
    #expect(world.model.alert?.message == "At most 2 attachments per message.")

    // What was typed while the refused send ran stays, after the refused text.
    let gate = Gate()
    world.messaging.data.with { $0.sendGate = gate }
    let sending = Task { await world.model.send() }
    await eventually { world.messaging.data.with { $0.sends.count } == 2 }

    world.model.setDraftText("Typed meanwhile")
    await gate.open()
    await sending.value
    #expect(world.model.draft.text == "Hello\nTyped meanwhile")
}

@MainActor
@Test func aMessageThePhoneCouldNotDeliverIsRetried() async {
    let world = World.started(conversations: [conversation(1, [alex])])
    world.messaging.data.with {
        $0.messages[1] = [message(7, at: 1_000, incoming: false, kind: .pendingSend, sendState: .failed(reason: .noService))]
    }
    world.model.select(1)
    #expect(world.model.transcript.first?.message.sendState == .failed(reason: .noService))

    await world.model.retry(7)
    #expect(world.messaging.data.with { $0.retries } == [7])
    #expect(world.model.retrying.isEmpty)
    #expect(world.model.alert == nil)

    world.messaging.data.with { $0.retryResult = .failure(.NotAvailable(reason: "no phone")) }
    await world.model.retry(7)
    #expect(world.model.alert?.title == "Message Not Sent")
}

@MainActor
@Test func eachConversationKeepsItsOwnDraft() {
    let world = World.started(conversations: [conversation(1, [alex]), conversation(2, [minji])])
    world.model.select(1)
    world.model.setDraftText("for Alex")

    world.model.select(2)
    #expect(world.model.draft.text.isEmpty)
    world.model.setDraftText("for Minji")
    world.model.select(1)
    #expect(world.model.draft.text == "for Alex")
}

@MainActor
@Test func whitespaceOrNothingCannotBeSent() async {
    let world = World.started(conversations: [conversation(1, [alex])])
    world.model.select(1)
    world.model.setDraftText("  \n ")
    #expect(!world.model.canSubmit)

    await world.model.send()
    #expect(world.messaging.data.with { $0.sends.isEmpty })
}

@MainActor
@Test func attachmentsAreImagesOnlyAndTwoAtMost() {
    let world = World.started(conversations: [conversation(1, [alex]), conversation(2, [minji], attachmentsAllowed: false)])
    world.model.select(1)

    #expect(world.model.addAttachments([image("a.png"), image("b.png"), image("c.png")]))
    #expect(world.model.draft.attachments.map(\.name) == ["a.png", "b.png"])
    #expect(world.model.alert?.message == "A message can have up to 2 images.")
    #expect(!world.model.canAddAttachments)

    world.model.alert = nil
    world.model.removeAttachment(world.model.draft.attachments[0].id)
    #expect(world.model.draft.attachments.map(\.name) == ["b.png"])
    #expect(world.model.canAddAttachments)

    let pdf = MessagesAttachment(name: "doc.pdf", contentType: "application/pdf", data: Data([1]))
    #expect(!world.model.addAttachments([pdf]))
    #expect(world.model.alert?.message == "Only images can be added to a message.")
    #expect(world.model.draft.attachments.map(\.name) == ["b.png"])

    // Pasted data without a type: the engine detects it from the bytes.
    world.model.alert = nil
    #expect(world.model.addAttachments([MessagesAttachment(name: "", contentType: nil, data: Data([1, 2]))]))
    #expect(world.model.draft.attachments.count == 2)

    world.model.select(2)
    #expect(!world.model.canAddAttachments)
    #expect(!world.model.addAttachments([image()]))
    #expect(world.model.alert?.message == "This conversation can't take attachments.")
    #expect(world.model.draft.attachments.isEmpty)
}

@MainActor
@Test func theComposerFollowsTheConversation() {
    let world = World.started(conversations: [
        conversation(1, [alex], maxTextLength: 5),
        conversation(2, [alex, minji], canReply: false),
    ])
    #expect(!world.model.composer.isEnabled, "no conversation shown")

    world.model.select(1)
    #expect(world.model.composer.isEnabled)
    #expect(world.model.composer.placeholder == "Text Message")

    world.model.setDraftText("Hi 😀 there")
    #expect(world.model.draft.text == "Hi 😀", "five UTF-16 code units, the emoji kept whole")

    world.model.select(2)
    #expect(!world.model.composer.isEnabled)
    #expect(world.model.composer.placeholder == "You can't reply to this conversation")

    world.model.setDraftText("Hello")
    #expect(!world.model.canSubmit)
}

@MainActor
@Test func theSimPickerNeedsTwoSims() {
    let world = World.started(conversations: [conversation(1, [alex], activeSubscription: 4)])
    world.model.select(1)
    #expect(!world.model.showsSimPicker)

    world.messaging.data.with {
        $0.subscriptions = [
            SimSubscription(id: 3, simSlotIndex: 0, name: "Home", number: "", isDefaultSms: true),
            SimSubscription(id: 4, simSlotIndex: 1, name: "", number: "", isDefaultSms: false),
        ]
    }
    world.post(.contentChanged(threads: []))
    #expect(world.model.showsSimPicker)
    #expect(world.model.activeSubscription?.id == 4)

    world.model.selectSubscription(3)
    #expect(world.messaging.data.with { $0.simChoices.map(\.subscriptionId) } == [3])

    world.model.startNewMessage()
    #expect(!world.model.showsSimPicker, "a new message sends from the default SIM")
}

@MainActor
@Test func theBluetoothGateIsExplainedAboveTheComposer() {
    let world = World.started(conversations: [conversation(1, [alex])], state: messagingState(proximity: .bluetoothOff))
    #expect(world.model.bluetoothNotice == nil, "no composer shown")

    world.model.select(1)
    #expect(world.model.bluetoothNotice == "Bluetooth is off. Turn it on to send messages from this Mac.")
    #expect(world.model.composer.isEnabled, "the phone decides; the line says why it may refuse")

    world.messaging.data.with { $0.state = messagingState(proximity: .connected) }
    world.model.setWindowFocused(true)
    #expect(world.model.bluetoothNotice == nil, "focusing the window reads the Bluetooth state again")
}

// MARK: - The list

@MainActor
@Test func theSearchMatchesNamesAddressesAndPreviewsOfListedConversations() {
    let world = World.started(conversations: [
        conversation(1, [alex], preview: ConversationPreviewLine(body: "Lunch tomorrow?", attachment: false, from: alex)),
        conversation(2, [minji], preview: ConversationPreviewLine(body: "좋아", attachment: false, from: nil)),
        conversation(3, [unknown], preview: ConversationPreviewLine(body: "Your parcel arrived", attachment: false, from: unknown)),
        conversation(4, [person("+15559999", "Alex Hidden")], hidden: true),
    ])
    #expect(world.model.visibleConversations.map(\.threadId) == [1, 2, 3])
    #expect(world.model.hiddenConversations.map(\.threadId) == [4])

    world.model.searchText = "alex"
    #expect(world.model.visibleConversations.map(\.threadId) == [1])
    #expect(world.model.hiddenConversations.map(\.threadId) == [4], "a hidden conversation is found too, apart")

    world.model.searchText = "민지"
    #expect(world.model.visibleConversations.map(\.threadId) == [2])
    #expect(world.model.hiddenConversations.isEmpty)

    world.model.searchText = "1588"
    #expect(world.model.visibleConversations.map(\.threadId) == [3])
    world.model.searchText = "PARCEL"
    #expect(world.model.visibleConversations.map(\.threadId) == [3])
    world.model.searchText = " "
    #expect(world.model.visibleConversations.count == 3)
}

@MainActor
@Test func theBadgeCountsUnreadMessagesOfListedConversations() {
    let world = World.started(conversations: [
        conversation(1, [alex], unread: 2),
        conversation(2, [minji], unread: 1, muted: true),
        conversation(3, [unknown], unread: 5, hidden: true),
    ])
    #expect(world.model.unreadCount == 3)

    world.messaging.data.with { $0.state = messagingState(pcConsent: false) }
    world.post(.stateChanged)
    #expect(world.model.unreadCount == 0, "no badge while messages are off on this Mac")

    world.messaging.data.with { $0.state = messagingState(permission: false) }
    world.post(.stateChanged)
    #expect(world.model.unreadCount == 0)
}

@MainActor
@Test func theListStatusFollowsConsentPermissionAndSync() {
    let world = World.started(state: messagingState(syncing: true, upToDate: false))
    #expect(world.model.listStatus == .syncing)

    world.messaging.data.with { $0.state = messagingState(syncing: false, upToDate: false) }
    world.post(.stateChanged)
    #expect(world.model.listStatus == .empty)

    // The sync after the permission prompt shows the syncing state with the anchors up to date.
    world.messaging.data.with { $0.state = messagingState(syncing: true, upToDate: true) }
    world.post(.stateChanged)
    #expect(world.model.listStatus == .syncing)

    world.messaging.data.with { $0.conversations = [conversation(1, [alex], hidden: true)] }
    world.post(.contentChanged(threads: []))
    #expect(world.model.listStatus == .conversations, "a hidden conversation is listed in its own section")

    world.messaging.data.with {
        $0.conversations = [conversation(1, [alex])]
        $0.state = messagingState(syncing: true, upToDate: false)
    }
    world.post(.contentChanged(threads: []))
    world.post(.stateChanged)
    #expect(world.model.listStatus == .conversations, "a sync does not hide what is there")

    world.messaging.data.with { $0.state = messagingState(pcConsent: false, permission: false) }
    world.post(.stateChanged)
    #expect(world.model.listStatus == .turnedOff)
    #expect(world.model.isGated)
}

@MainActor
@Test func writingToAPersonShowsTheirConversationElseANewMessageToThemAlone() {
    let world = World.started(conversations: [
        conversation(1, [person("+15550100", "Amy")]),
        conversation(2, [person("+15550100", "Amy"), person("+15550200")]),
    ])
    world.model.write(to: MessagesRecipient(address: "+1 555 0100", name: "Amy"))
    #expect(world.model.detail == .conversation(1), "the conversation of that person alone, not the group")

    world.model.setDraftText("Hi")
    world.model.write(to: MessagesRecipient(address: "+15550300", name: "Bo"))
    #expect(world.model.detail == .newMessage)
    #expect(world.model.recipients == [MessagesRecipient(address: "+15550300", name: "Bo")])

    world.model.setDraftText("Hello")
    world.model.write(to: MessagesRecipient(address: "+15550400", name: nil))
    #expect(world.model.recipients == [MessagesRecipient(address: "+15550400", name: nil)], "a new person replaces the recipients")
    #expect(world.model.draft.text == "Hello", "what is typed stays")
}

@MainActor
@Test func writingToAPersonTurnsMessagesOn() {
    let world = World.started(state: messagingState(pcConsent: false))
    world.model.write(to: MessagesRecipient(address: "+15550100", name: nil))
    #expect(world.messaging.data.with { $0.settings }.first?.pcConsent == true)
    #expect(world.model.detail == .newMessage)
}

@MainActor
@Test func turningMessagesOnWritesTheConsentAndKeepsTheOtherSettings() {
    let world = World.started(state: messagingState(pcConsent: false, sendMms: false))
    world.model.turnOnMessages()

    let written = world.messaging.data.with { $0.settings }
    #expect(written.count == 1)
    #expect(written.first?.pcConsent == true)
    #expect(written.first?.sendMms == false)
    #expect(world.model.listStatus == .empty)
}

@MainActor
@Test func aDeniedPermissionRequestIsReported() async {
    let world = World.started(state: messagingState(permission: false))
    world.messaging.data.with { $0.permissionOutcome = .denied }

    await world.model.requestPermission()

    #expect(world.model.alert?.title == "Messages Not Allowed")
    #expect(world.model.alert?.message == "Your phone denied the request.")
    #expect(!world.model.isRequestingPermission)

    world.model.alert = nil
    world.messaging.data.with { $0.permissionOutcome = .accepted }
    await world.model.requestPermission()
    #expect(world.model.alert == nil)
}

@MainActor
@Test func hidingIsUndoableAndMutingKeepsTheOtherFlag() {
    let world = World.started(conversations: [conversation(1, [alex]), conversation(2, [minji], muted: true)])
    let undo = UndoManager()
    undo.groupsByEvent = false
    world.model.select(1)

    undo.beginUndoGrouping()
    world.model.setHidden(true, threadId: 1, undoManager: undo)
    undo.endUndoGrouping()

    #expect(world.messaging.data.with { $0.flags.last } == "1 muted:false hidden:true")
    #expect(world.model.visibleConversations.map(\.threadId) == [2])
    #expect(world.model.hiddenConversations.map(\.threadId) == [1])
    #expect(world.model.selection == nil, "the hidden conversation is no longer shown")
    #expect(undo.undoActionName == "Hide Conversation")

    undo.undo()
    #expect(world.messaging.data.with { $0.flags.last } == "1 muted:false hidden:false")
    #expect(world.model.visibleConversations.map(\.threadId) == [1, 2])
    #expect(undo.redoActionName == "Hide Conversation")

    world.model.setMuted(false, threadId: 2)
    #expect(world.messaging.data.with { $0.flags.last } == "2 muted:false hidden:false")
    world.model.setMuted(true, threadId: 1)
    #expect(world.messaging.data.with { $0.flags.last } == "1 muted:true hidden:false")
}

@MainActor
@Test func aHiddenConversationCanBeShownAgainAndWritingToItShowsIt() async {
    let world = World.started(conversations: [conversation(1, [alex], hidden: true), conversation(2, [minji], hidden: true)])
    let undo = UndoManager()
    undo.groupsByEvent = false

    undo.beginUndoGrouping()
    world.model.setHidden(false, threadId: 1, undoManager: undo)
    undo.endUndoGrouping()
    #expect(world.model.visibleConversations.map(\.threadId) == [1])
    #expect(undo.undoActionName == "Show Conversation")

    undo.undo()
    #expect(world.model.hiddenConversations.map(\.threadId) == [1, 2])
    #expect(undo.redoActionName == "Show Conversation")

    // A hidden conversation can be opened; sending to it shows it again.
    world.model.select(2)
    world.model.setDraftText("Back again")
    await world.model.send()
    #expect(world.messaging.data.with { $0.flags.last } == "2 muted:false hidden:false")
    #expect(world.model.visibleConversations.map(\.threadId) == [2])

    // So does writing to its people from a new message: the hidden conversation takes the composer's
    // place and is listed again.
    world.messaging.data.with { $0.unsent = [unsentMessage(50, to: [person("+15550101")], at: 9_000, state: .sending)] }
    world.model.startNewMessage()
    world.model.setRecipientQuery("+1 555 0101")
    world.model.setDraftText("Hi Alex")
    await world.model.send()

    #expect(world.model.selection == 1)
    #expect(world.messaging.data.with { $0.flags.last } == "1 muted:false hidden:false")
    #expect(world.model.hiddenConversations.isEmpty)
    #expect(world.calls.filter { $0 == "replaced 1" }.count == 1)
}

// MARK: - Downloads

@MainActor
@Test func aDownloadRunsUntilTheBytesArriveOrItFails() async {
    let part = MessagePartRef.mmsPart(id: 42)
    let waiting = MessagePart(part: part, contentType: "image/jpeg", name: "", size: 1000, state: .downloadable, hasBytes: false)
    let arrived = MessagePart(part: part, contentType: "image/jpeg", name: "", size: 1000, state: .image, hasBytes: true)
    let world = World.started(
        conversations: [conversation(1, [alex])],
        timing: MessagesTiming(searchDelay: .zero, downloadWait: .milliseconds(150))
    )

    world.messaging.data.with { $0.messages[1] = [message(10, at: 1_000, body: "", kind: .mms, parts: [waiting])] }
    world.model.select(1)
    let gate = Gate()
    world.messaging.data.with { $0.downloadGate = gate }

    let downloading = Task { await world.model.download(part) }
    await eventually { world.messaging.data.with { $0.downloads.count } == 1 }
    #expect(world.model.downloading == [part])

    await gate.open()
    await downloading.value
    #expect(world.model.downloading == [part], "the phone said yes; the bytes are still to come")

    world.messaging.data.with { $0.messages[1] = [message(10, at: 1_000, body: "", kind: .mms, parts: [arrived])] }
    world.post(.contentChanged(threads: [1]))
    #expect(world.model.downloading.isEmpty, "they came")
    #expect(world.model.alert == nil)

    // A failed request says why at once.
    world.messaging.data.with {
        $0.messages[1] = [message(10, at: 1_000, body: "", kind: .mms, parts: [waiting])]
        $0.downloadGate = nil
        $0.downloadResult = .success(.timedOut)
    }
    world.post(.contentChanged(threads: [1]))
    await world.model.download(part)
    #expect(world.model.downloading.isEmpty)
    #expect(world.model.alert?.title == "Attachment Not Downloaded")
    #expect(world.model.alert?.message == "Your phone didn't respond.")

    // Bytes that never come end the wait with a message.
    world.model.alert = nil
    world.messaging.data.with { $0.downloadResult = .success(.success) }
    await world.model.download(part)
    #expect(world.model.downloading == [part])
    await eventually { world.model.alert != nil }
    #expect(world.model.downloading.isEmpty)
    #expect(world.model.alert?.message == "Your phone didn't send the attachment.")

    world.model.alert = nil
    world.messaging.data.with { $0.downloadResult = .failure(.NotAvailable(reason: "the part is not offered for download")) }
    await world.model.download(part)
    #expect(world.model.alert?.title == "Attachment Not Downloaded")
}

@MainActor
@Test func aDownloadWaitRunsOnWhileAnotherConversationIsShown() async {
    // Differs from Phone Link on Windows: after the phone said yes the part shows as downloading
    // until its bytes come or the wait runs out, whichever conversation is shown meanwhile.
    let part = MessagePartRef.mmsPart(id: 42)
    let waiting = MessagePart(part: part, contentType: "image/jpeg", name: "", size: 1000, state: .downloadable, hasBytes: false)
    let arrived = MessagePart(part: part, contentType: "image/jpeg", name: "", size: 1000, state: .image, hasBytes: true)
    let world = World.started(
        conversations: [conversation(1, [alex]), conversation(2, [minji])],
        timing: MessagesTiming(searchDelay: .zero, downloadWait: .milliseconds(150))
    )

    world.messaging.data.with { $0.messages[1] = [message(10, at: 1_000, body: "", kind: .mms, parts: [waiting])] }
    world.model.select(1)
    await world.model.download(part)
    #expect(world.model.downloading == [part])

    world.model.select(2)
    #expect(world.model.downloading == [part], "switching conversations does not end the wait")

    // The bytes come while conversation 2 is shown: the wait ends without a message.
    world.messaging.data.with {
        $0.messages[1] = [message(10, at: 1_000, body: "", kind: .mms, parts: [arrived])]
        $0.partBytes[part] = pngData
    }
    world.post(.contentChanged(threads: [1]))
    #expect(world.model.downloading == [part], "conversation 1 is not shown")

    await eventually { world.model.downloading.isEmpty }
    #expect(world.model.alert == nil, "the bytes came")
    #expect(world.messaging.data.with { $0.partReads } == [part], "the engine said so")

    // Bytes that never come are reported in whichever conversation is shown.
    world.messaging.data.with {
        $0.messages[1] = [message(10, at: 1_000, body: "", kind: .mms, parts: [waiting])]
        $0.partBytes[part] = nil
    }
    world.model.select(1)
    await world.model.download(part)
    world.model.select(2)

    await eventually { world.model.alert != nil }
    #expect(world.model.downloading.isEmpty)
    #expect(world.model.alert?.message == "Your phone didn't send the attachment.")
}

// MARK: - Pictures

@MainActor
@Test func picturesAreReadOnceAndAFailedReadIsTriedAgain() async throws {
    let world = World.started()
    let part = MessagePartRef.mmsPart(id: 5)
    world.messaging.data.with { $0.partBytes[part] = pngData }

    #expect(await world.model.image(of: .part(part, size: 100), maxPixels: 64) != nil)
    #expect(await world.model.image(of: .part(part, size: 100), maxPixels: 64) != nil)
    #expect(world.messaging.data.with { $0.partReads } == [part], "kept once decoded")

    // A contact photo that could not be read is read again next time, not kept as "no photo".
    world.contacts.data.with { $0.photoError = .Store(message: "busy") }
    #expect(await world.model.image(of: .contact(7), maxPixels: 64) == nil)

    world.contacts.data.with {
        $0.photoError = nil
        $0.photos[7] = pngData
    }
    #expect(await world.model.image(of: .contact(7), maxPixels: 64) != nil)
    #expect(world.contacts.data.with { $0.photoReads } == [7, 7])

    // A contact without a photo is not asked again; a change of contacts asks again.
    #expect(await world.model.image(of: .contact(8), maxPixels: 64) == nil)
    #expect(await world.model.image(of: .contact(8), maxPixels: 64) == nil)
    #expect(world.contacts.data.with { $0.photoReads } == [7, 7, 8])

    world.hub.post(.contacts(change: .recordsChanged))
    #expect(await world.model.image(of: .contact(8), maxPixels: 64) == nil)
    #expect(world.contacts.data.with { $0.photoReads } == [7, 7, 8, 8])

    let draft = MessagesAttachment(name: "", contentType: "image/png", data: pngData)
    #expect(await world.model.image(of: draft, maxPixels: 64) != nil)
    #expect(try await world.model.partData(part) == pngData)
}

@MainActor
@Test func aConversationWhoseMessagesCannotBeReadOffersTryAgain() {
    let world = World.started(conversations: [conversation(1, [alex]), conversation(2, [minji])])
    world.messaging.data.with {
        $0.messages[1] = [message(10, at: 1_000)]
        $0.messagesError = .Store(message: "disk I/O error")
    }
    world.model.select(1)
    #expect(world.model.transcriptFailed)
    #expect(world.model.transcript.isEmpty)

    world.messaging.data.with { $0.messagesError = nil }
    world.model.reloadTranscript()
    #expect(!world.model.transcriptFailed)
    #expect(world.model.transcript.map(\.message.id) == [10])

    // A later failure keeps what was read.
    world.messaging.data.with { $0.messagesError = .Store(message: "disk I/O error") }
    world.post(.contentChanged(threads: [1]))
    #expect(!world.model.transcriptFailed)
    #expect(world.model.transcript.map(\.message.id) == [10])
}

// MARK: - Mark-as-read triggers

@MainActor
@Test func selectionFocusAndTheSectionReachTheEngine() {
    let world = World.started(conversations: [conversation(1, [alex]), conversation(2, [minji])])

    world.model.setMessagesViewActive(true)
    world.model.setWindowFocused(true)
    world.model.select(1)
    world.model.select(1)
    world.model.select(2)
    world.model.select(nil)
    world.model.setWindowFocused(false)
    world.model.setMessagesViewActive(false)

    #expect(world.calls == [
        "view true", "focused true", "selected 1", "selected 2", "selected none", "focused false", "view false",
    ])
}

@MainActor
@Test func aRestartedEngineLearnsWhatTheWindowShows() {
    let world = World.started(conversations: [conversation(1, [alex])])
    world.model.select(1)
    #expect(world.calls == ["selected 1"])

    world.hub.postStarted()
    #expect(world.calls == ["selected 1", "selected 1"])

    world.model.setMessagesViewActive(true)
    world.model.setWindowFocused(true)
    world.hub.postStarted()
    #expect(world.calls.suffix(3) == ["view true", "focused true", "selected 1"])
}

@MainActor
@Test func whileTheGateHidesTheConversationTheEngineSeesNoSelection() {
    let world = World.started(conversations: [conversation(1, [alex])])
    world.model.setMessagesViewActive(true)
    world.model.setWindowFocused(true)
    world.model.select(1)

    world.messaging.data.with { $0.state = messagingState(pcConsent: false) }
    world.post(.stateChanged)
    #expect(world.model.isGated)
    #expect(world.calls.last == "selected none", "no trigger can mark what the window does not show")

    world.model.setWindowFocused(false)
    world.model.setWindowFocused(true)
    #expect(world.calls.suffix(3) == ["selected none", "focused false", "focused true"], "no selection while gated")

    world.messaging.data.with { $0.state = messagingState() }
    world.post(.stateChanged)
    #expect(world.calls.last == "selected 1", "shown again once the gate lifts")
    #expect(world.model.selection == 1)

    world.messaging.data.with { $0.state = messagingState(permission: false) }
    world.post(.permissionChanged(granted: false))
    #expect(world.calls.last == "selected none")
}

// MARK: - New message

@MainActor
@Test func newMessageRecipientsComeFromSuggestionsAndTypedAddresses() async {
    let world = World.started(conversations: [conversation(1, [alex])])
    world.contacts.data.with {
        $0.book = [
            suggestion(7, "Alex Kim", "+1 555-0101", photo: true),
            suggestion(8, "Alexandra", "+1 555-0199"),
        ]
        $0.photos = [7: pngData]
    }
    world.model.select(1)

    world.model.startNewMessage()
    #expect(world.model.isComposing)
    #expect(world.model.selection == nil)
    #expect(world.calls.last == "selected none")
    #expect(world.model.showsNewMessageRow)
    #expect(world.model.listSelection == .newMessage)

    world.model.setRecipientQuery("alex")
    #expect(world.contacts.data.with { $0.queries.last?.query } == "alex")
    #expect(world.contacts.data.with { $0.queries.last?.limit } == 8)
    #expect(world.model.suggestions.map(\.contactId) == [7, 8])
    #expect(await world.model.image(of: .contact(7), maxPixels: 56) != nil)

    world.model.submitRecipientQuery()
    #expect(world.model.recipients == [MessagesRecipient(address: "+1 555-0101", name: "Alex Kim")])
    #expect(world.model.recipientQuery.isEmpty)
    #expect(world.model.suggestions.isEmpty)

    world.model.setRecipientQuery("alex")
    #expect(world.model.suggestions.map(\.contactId) == [8], "a recipient is not suggested again")
    world.model.addRecipient(world.model.suggestions[0])

    world.model.setRecipientQuery("010-1234-5678")
    #expect(world.model.suggestions.isEmpty)
    world.model.submitRecipientQuery()

    world.model.setRecipientQuery("Bob")
    world.model.submitRecipientQuery()
    #expect(world.model.recipientQuery == "Bob", "a name without a contact is not a recipient")
    #expect(world.model.recipients.map(\.title) == ["Alex Kim", "Alexandra", "010-1234-5678"])

    world.model.removeRecipient(world.model.recipients[1])
    #expect(world.model.recipients.map(\.title) == ["Alex Kim", "010-1234-5678"])
    #expect(world.model.newMessagePreview == "To: Alex Kim, 010-1234-5678")

    world.model.startNewMessage()
    #expect(world.model.recipients.count == 2, "an open new message stays as it is")
}

@MainActor
@Test func aNewMessageIsKeptWhenAnotherConversationIsShownAndEscapeClosesItOnlyEmpty() {
    let world = World.started(conversations: [conversation(1, [alex]), conversation(2, [minji])])
    world.model.select(2)
    world.model.startNewMessage()
    #expect(world.model.cancelNewMessage(), "an empty new message closes")
    #expect(world.model.selection == 2, "the conversation shown before comes back")
    #expect(!world.model.showsNewMessageRow)

    world.model.startNewMessage()
    world.model.setRecipientQuery("010-1234-5678")
    world.model.submitRecipientQuery()
    world.model.setDraftText("Half a thought")

    world.navigator.showConversation(1)
    world.model.select(1)
    #expect(world.model.selection == 1)
    #expect(world.model.showsNewMessageRow, "a new message that holds something keeps its row")

    world.model.selectItem(.newMessage)
    #expect(world.model.recipients.map(\.address) == ["010-1234-5678"])
    #expect(world.model.draft.text == "Half a thought")
    #expect(!world.model.cancelNewMessage(), "Escape does not throw away what was written")
    #expect(world.model.isComposing)

    world.model.discardNewMessage()
    #expect(!world.model.showsNewMessageRow)
    #expect(world.model.selection == 1)

    world.model.startNewMessage()
    #expect(world.model.recipients.isEmpty)
    #expect(world.model.draft.text.isEmpty)
}

@MainActor
@Test func aNewMessageShowsWhatItSentUntilThePhoneCreatesItsConversation() async {
    let world = World.started(conversations: [conversation(1, [alex])])
    let gate = Gate()
    world.messaging.data.with {
        $0.sendResult = .success(SendResult(sendId: 31, state: .sent, outcome: .success))
        $0.sendGate = gate
    }
    world.model.startNewMessage()
    world.model.setRecipientQuery("010-5555-6666")
    world.model.setDraftText("Hello")

    let sending = Task { await world.model.send() }
    await eventually { world.messaging.data.with { $0.sends.count } == 1 }
    #expect(world.messaging.data.with { $0.sends.first?.target } == .newConversation(recipients: ["010-5555-6666"]))
    #expect(world.model.recipientsLocked, "the recipients lock when the send starts")
    #expect(world.model.recipients.map(\.address) == ["010-5555-6666"], "a typed number is sent to as typed")
    #expect(world.model.draft.text.isEmpty)

    world.model.removeRecipient(world.model.recipients[0])
    #expect(world.model.recipients.count == 1, "locked")

    // The engine stores the send, then the phone syncs the conversation before the send returns.
    world.messaging.data.with {
        $0.unsent = [unsentMessage(31, to: [person("010-5555-6666")], at: 2_000, state: .sending)]
    }
    world.post(.sendChanged(sendId: 31))
    #expect(world.model.transcript.map(\.message.id) == [31])

    world.model.setDraftText("Are you there?")
    world.messaging.data.with {
        $0.conversations.append(conversation(9, [person("+82 10-5555-6666")]))
        $0.messages[9] = [message(100, at: 2_000, incoming: false, body: "Hello")]
    }
    world.post(.contentChanged(threads: [5]))
    #expect(!world.model.isComposing, "adopted from any content change, whichever threads it names")
    #expect(world.model.selection == 9)
    #expect(world.calls.last == "replaced 9")
    #expect(!world.calls.contains("selected 9"), "the replacement is the selection")
    #expect(world.model.transcript.map(\.message.id) == [100, 31], "the send shows in its conversation until the phone confirms it")
    #expect(world.model.draft.text == "Are you there?", "what was typed meanwhile moves along")

    await gate.open()
    await sending.value
    #expect(world.model.selection == 9)

    world.messaging.data.with { $0.unsent = [] }
    world.post(.sendChanged(sendId: 31))
    #expect(world.model.transcript.map(\.message.id) == [100])
}

@MainActor
@Test func aFailedNewMessageStaysInTheListWithItsStateAndRetry() async {
    let world = World.started(conversations: [conversation(1, [alex])])
    world.messaging.data.with { $0.sendResult = .success(SendResult(sendId: 40, state: .failed(reason: .noService), outcome: .failure)) }
    world.model.startNewMessage()
    world.model.setRecipientQuery("010-7777-8888")
    world.model.setDraftText("Running late")
    world.messaging.data.with {
        $0.unsent = [unsentMessage(40, to: [person("010-7777-8888", "Sam")], at: 5_000, body: "Running late", state: .failed(reason: .noService))]
    }

    await world.model.send()

    // Showing another conversation leaves the failed message in the list.
    world.model.select(1)
    #expect(world.model.listedUnsent.map(\.id) == [["010-7777-8888"]])
    let row = world.model.listedUnsent[0]
    #expect(MessagesFormat.title(row) == "To: Sam")
    #expect(MessagesFormat.preview(row) == "Not Delivered · No service")
    #expect(world.model.listStatus == .conversations)

    world.model.selectItem(.unsent(row.id))
    #expect(world.model.isComposing && world.model.recipientsLocked)
    #expect(world.model.listSelection == .unsent(row.id))
    #expect(world.model.transcript.map(\.message.sendState) == [.failed(reason: .noService)])

    await world.model.retry(40)
    #expect(world.messaging.data.with { $0.retries } == [40])

    // Another message to the same people goes with them.
    world.model.setDraftText("Still coming")
    await world.model.send()
    #expect(world.messaging.data.with { $0.sends.last?.target } == .newConversation(recipients: ["010-7777-8888"]))

    // Once their conversation exists, the sends show there and the row goes.
    world.messaging.data.with { $0.conversations.append(conversation(12, [person("+821077778888", "Sam")])) }
    world.post(.contentChanged(threads: [12]))
    #expect(world.model.listedUnsent.isEmpty)
    #expect(world.model.selection == 12)
    #expect(world.model.transcript.map(\.message.id) == [40])
}

@MainActor
@Test func aRefusedNewMessageGetsItsRecipientsAndDraftBack() async {
    let world = World.started()
    world.messaging.data.with { $0.sendResult = .failure(.NotAvailable(reason: "more participants than the carrier allows")) }
    world.model.startNewMessage()
    world.model.setRecipientQuery("010-5555-6666")
    world.model.setDraftText("Hello")
    await world.model.send()

    #expect(world.model.detail == .newMessage)
    #expect(!world.model.recipientsLocked)
    #expect(world.model.recipients.map(\.address) == ["010-5555-6666"])
    #expect(world.model.draft.text == "Hello")
    #expect(world.model.alert?.message == "Not available: more participants than the carrier allows.")
}

@MainActor
@Test func aNewMessageFollowsTheDefaultSimLimits() async {
    let world = World.started()
    world.messaging.data.with {
        $0.limits = NewMessageLimits(attachmentsAllowed: false, groupAllowed: true, maxTextLength: 5, maxRecipients: 2, activeSubscription: 1)
    }
    world.post(.stateChanged)

    world.model.startNewMessage()
    #expect(!world.model.composer.canAttach)
    #expect(!world.model.canAddAttachments)

    world.model.setDraftText("Hello there")
    #expect(world.model.draft.text == "Hello")

    world.model.setRecipientQuery("010-1111-2222")
    world.model.submitRecipientQuery()
    world.model.setRecipientQuery("010-3333-4444")
    world.model.submitRecipientQuery()
    #expect(world.model.recipients.count == 2)
    #expect(!world.model.canAddRecipient, "the SIM's recipient limit")

    // A SIM that sends no group messages takes one recipient.
    world.messaging.data.with { $0.limits.groupAllowed = false }
    world.post(.contentChanged(threads: []))
    #expect(world.model.recipientCap == 1)
    #expect(!world.model.composer.isEnabled)
    #expect(world.model.composer.placeholder == "This SIM sends to one person at a time")

    await world.model.send()
    #expect(world.messaging.data.with { $0.sends.isEmpty })

    world.model.removeRecipient(world.model.recipients[1])
    #expect(world.model.composer.isEnabled)

    world.model.setRecipientQuery("010-5555-6666")
    #expect(world.model.canSubmit)
    await world.model.send()
    #expect(world.model.alert?.message == "This SIM sends to one person at a time.")
    #expect(world.messaging.data.with { $0.sends.isEmpty })
}

@MainActor
@Test func aNewMessageNeedsARecipient() async {
    let world = World.started()
    world.model.startNewMessage()
    world.model.setDraftText("Hello")
    #expect(!world.model.canSubmit)

    world.model.setRecipientQuery("Bob")
    #expect(world.model.canSubmit, "what is typed may still be an address")

    await world.model.send()
    #expect(world.messaging.data.with { $0.sends.isEmpty })
    #expect(world.model.alert?.message == "“Bob” isn't a phone number or an email address.")
    #expect(world.model.draft.text == "Hello")
}

@MainActor
@Test func contactsThatCannotBeSearchedOfferTheirActionAndSuggestNothing() async {
    let world = World.started()
    world.contacts.data.with {
        $0.state.access = .deviceConsentRequired
        $0.book = [suggestion(7, "Alex Kim", "+1 555-0101")]
    }

    world.model.startNewMessage()
    #expect(world.model.contactsNotice == .allowOnPhone)

    world.model.setRecipientQuery("alex")
    #expect(world.model.suggestions.isEmpty)
    #expect(world.contacts.data.with { $0.queries.isEmpty }, "contacts the phone has not allowed are not searched")

    world.contacts.data.with { $0.permission = ContactsPermissionResult(outcome: .success, timedOut: true, sync: nil) }
    await world.model.requestContactsPermission()
    #expect(world.model.alert?.message == "Your phone didn't respond.")

    world.contacts.data.with { $0.state.access = .pcConsentRequired }
    world.hub.post(.contacts(change: .stateChanged))
    #expect(world.model.contactsNotice == .turnOn)
    #expect(world.model.suggestions.isEmpty)

    world.model.turnOnContacts()
    #expect(world.contacts.data.with { $0.consents } == [true])
    #expect(world.model.contactsNotice == nil)
    #expect(world.model.suggestions.map(\.contactId) == [7], "searched once they may be used")

    world.contacts.data.with {
        $0.state.access = .deviceConsentDenied
        $0.state.capable = false
    }
    world.hub.post(.contacts(change: .stateChanged))
    #expect(world.model.contactsNotice == nil, "a phone without contacts sync offers nothing")
    #expect(world.model.suggestions.isEmpty)
}

@MainActor
@Test func contactChangesRenameConversationsAndRefreshSuggestions() {
    let world = World.started(conversations: [conversation(1, [person("+15550101")])])
    world.model.select(1)
    world.model.startNewMessage()
    world.model.setRecipientQuery("Alex")
    #expect(world.model.suggestions.isEmpty)

    world.messaging.data.with { $0.conversations = [conversation(1, [alex])] }
    world.contacts.data.with { $0.book = [suggestion(7, "Alex Kim", "+15550101")] }
    world.hub.post(.contacts(change: .recordsChanged))

    #expect(world.model.conversations.first?.recipients.first?.name == "Alex Kim")
    #expect(world.model.suggestions.map(\.contactId) == [7])
}

// MARK: - The composer's undo

/// The composer field's delegate, as far as undo goes.
@MainActor
private final class UndoDelegate: NSObject, NSTextViewDelegate {
    let undo: MessagesComposerUndo

    init(_ undo: MessagesComposerUndo) {
        self.undo = undo
    }

    func undoManager(for view: NSTextView) -> UndoManager? {
        undo.undoManager
    }
}

@MainActor
@Test func replacingTheComposerTextInCodeClearsItsUndo() {
    let undo = MessagesComposerUndo()
    let delegate = UndoDelegate(undo)
    let textView = NSTextView(frame: NSRect(x: 0, y: 0, width: 200, height: 40))
    textView.allowsUndo = true
    textView.delegate = delegate
    #expect(textView.undoManager === undo.undoManager, "the text view's own stack, not the window's")

    textView.insertText("draft for A", replacementRange: NSRange(location: NSNotFound, length: 0))
    textView.breakUndoCoalescing()
    #expect(undo.undoManager.canUndo)

    undo.replaceText(of: textView, with: "B")
    #expect(!undo.undoManager.canUndo, "A's typing does not apply to B's draft")
    #expect(textView.string == "B")
}
