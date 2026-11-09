import AppKit
import Observation
import os
import PhoneLinkEngine

private let log = AppLog(category: "messages")

/// The Messages section: the phone's conversations, what this Mac sent to new recipients before the
/// phone created their conversation, the new message being written, the shown detail with its
/// transcript and composer, a draft per composer, and what the engine's mark-as-read triggers need
/// to hear from the window.
///
/// Engine reads run off the main actor (`MessagesReading`); a result a newer read of the same kind
/// superseded is dropped, and a failed read is logged while the last data stays. Settings and flags
/// are written synchronously. Requests to the phone are `async` (the views run them in a `Task`); the
/// user started them, so their failures become `alert`.
@MainActor
@Observable
public final class MessagesModel {
    /// At most this many images per message (composition rule 5).
    public static let maxAttachments = 2
    private static let suggestionLimit: UInt32 = 8

    // MARK: Engine data

    /// Every conversation, newest first, hidden ones included (the list shows them apart).
    public private(set) var conversations: [Conversation] = []
    /// What this Mac sent to new recipients that the phone has not confirmed yet, one entry per set of
    /// recipients, newest first. An entry whose recipients have a conversation shows in it.
    public private(set) var unsent: [MessagesUnsentConversation] = []
    /// `nil` until the engine has started.
    public private(set) var state: MessagingState?
    /// The phone's SIMs.
    public private(set) var subscriptions: [SimSubscription] = []
    /// `nil` until the engine has started.
    public private(set) var contactsState: ContactsState?
    /// What a new message may carry (the default SIM's limits); `nil` until read.
    public private(set) var newMessageLimits: NewMessageLimits?

    // MARK: Window state

    /// What the detail column shows.
    public private(set) var detail: MessagesDetail?
    /// The list's search; the list filters locally.
    public var searchText = ""
    /// The shown detail's messages.
    public private(set) var transcript: [MessagesTranscriptRow] = []
    /// The shown conversation's messages could not be read; `reloadTranscript()` tries again.
    public private(set) var transcriptFailed = false
    /// What the user types in the new message's "To:" field.
    public private(set) var recipientQuery = ""
    /// Contacts matching `recipientQuery`.
    public private(set) var suggestions: [ContactSuggestion] = []
    /// Failed sends being sent again.
    public private(set) var retrying: Set<Int64> = []
    public private(set) var isRequestingPermission = false
    public private(set) var isRequestingContacts = false
    /// The failure of an action the user started.
    public var alert: MessagesAlert?

    /// The new message's recipients (it keeps them while another detail is shown).
    private var newRecipients: [MessagesRecipient] = []
    private var drafts: [MessagesComposerTarget: MessagesDraft] = [:]
    /// Parts whose download request is under way.
    private var requestedDownloads: Set<MessagePartRef> = []
    /// Parts the phone agreed to send, until their bytes arrive: the number of the wait that runs. A
    /// wait belongs to its part, not to the transcript shown: it runs on while another conversation is
    /// shown.
    private var awaitedDownloads: [MessagePartRef: Int] = [:]

    /// The engine's messages of the conversation `transcriptOwner` names.
    @ObservationIgnored private var transcriptMessages: [Message] = []
    @ObservationIgnored private var transcriptOwner: MessagesDetail?
    /// A read of the owner's messages succeeded.
    @ObservationIgnored private var transcriptLoaded = false
    /// The conversation shown when the new message opened, shown again when Escape closes it.
    @ObservationIgnored private var beforeNewMessage: Int64?
    /// The selection the engine was last told (`.none`: not known).
    @ObservationIgnored private var reportedSelection: Int64?? = .none
    /// What the window last reported, told again to a new engine core.
    @ObservationIgnored private var messagesViewActive: Bool?
    @ObservationIgnored private var windowFocused: Bool?
    @ObservationIgnored private var generations = ReadGenerations()
    @ObservationIgnored private var suggestionSearch: Task<Void, Never>?
    @ObservationIgnored private var withoutImage: Set<String> = []
    /// A picture read failed since the engine started (logged once, since each cell asks again).
    @ObservationIgnored private var pictureFailureLogged = false
    /// Bumped when contacts change, so their photos are read again.
    @ObservationIgnored private var photosGeneration = 0
    @ObservationIgnored private var downloadWaits = 0
    /// Decoded pictures of transcripts, drafts, suggestions and avatars.
    private let images = NSCache<NSString, NSImage>()

    private let api: any MessagingApiProtocol
    private let contacts: any ContactsApiProtocol
    private let navigator: AppNavigator
    private let reads: any MessagesReading
    private let timing: MessagesTiming
    /// The clock and calendar times are shown with.
    public let time: MessagesTimeFormatter

    public convenience init(
        api: any MessagingApiProtocol,
        contacts: any ContactsApiProtocol,
        hub: EngineEventHub,
        navigator: AppNavigator,
        time: MessagesTimeFormatter = MessagesTimeFormatter()
    ) {
        self.init(
            api: api,
            contacts: contacts,
            hub: hub,
            navigator: navigator,
            time: time,
            reads: MessagesBackgroundReads(),
            timing: .live
        )
    }

    init(
        api: any MessagingApiProtocol,
        contacts: any ContactsApiProtocol,
        hub: EngineEventHub,
        navigator: AppNavigator,
        time: MessagesTimeFormatter,
        reads: any MessagesReading,
        timing: MessagesTiming
    ) {
        self.api = api
        self.contacts = contacts
        self.navigator = navigator
        self.time = time
        self.reads = reads
        self.timing = timing

        hub.onEvent { [weak self] event in
            self?.handle(event)
        }
        hub.onStart { [weak self] in
            self?.engineStarted()
        }
    }

    // MARK: - What the views show

    /// The sidebar badge: unread messages of the listed conversations; none while the list is replaced
    /// by the off or permission state.
    public var unreadCount: Int {
        guard gate == nil else { return 0 }
        return conversations.reduce(0) { sum, conversation in
            conversation.hidden ? sum : sum + Int(max(conversation.unread, 0))
        }
    }

    public var listStatus: MessagesListStatus {
        if let gate { return gate }
        if !conversations.isEmpty || !unsent.isEmpty || showsNewMessageRow { return .conversations }

        // The syncing state also marks the sync after the permission prompt, anchors up to date or
        // not (sync trigger (a)).
        if state?.syncing == true { return .syncing }
        return .empty
    }

    /// Messages are off on this Mac or not allowed on the phone: the list and the detail stand aside.
    public var isGated: Bool {
        gate != nil
    }

    /// The listed conversations: not hidden, and matching the search by a name, an address or the preview.
    public var visibleConversations: [Conversation] {
        let query = searchQuery
        return conversations.filter { !$0.hidden && (query.isEmpty || Self.matches($0, query)) }
    }

    /// The hidden conversations, in their own section of the list, matching the search as listed ones do.
    public var hiddenConversations: [Conversation] {
        let query = searchQuery
        return conversations.filter { $0.hidden && (query.isEmpty || Self.matches($0, query)) }
    }

    /// What was sent to new recipients that no conversation holds yet, newest first, matching the
    /// search by a recipient.
    public var listedUnsent: [MessagesUnsentConversation] {
        let query = searchQuery
        return unsent.filter { group in
            conversation(matching: group.addresses) == nil
                && (query.isEmpty || group.recipients.contains { Self.matches($0, query) })
        }
    }

    /// The new message being written has a row while it is shown or holds something.
    public var showsNewMessageRow: Bool {
        detail == .newMessage || newMessageHasContent
    }

    /// The new message row's line: its recipients, else its text.
    public var newMessagePreview: String {
        MessagesFormat.newMessagePreview(recipients: newRecipients, text: drafts[.newMessage]?.text ?? "")
    }

    /// The new message holds recipients, typed text or a draft; Escape does not close it then.
    public var newMessageHasContent: Bool {
        !newRecipients.isEmpty || !trimmed(recipientQuery).isEmpty || !(drafts[.newMessage]?.isEmpty ?? true)
    }

    /// The list's selection: the shown detail as a row.
    public var listSelection: MessagesListItem? {
        get {
            switch detail {
            case nil: nil
            case let .conversation(threadId): .conversation(threadId)
            case .newMessage: .newMessage
            case let .unsent(people): unsentGroup(people.map(\.address)).map { .unsent($0.id) }
            }
        }
        set { selectItem(newValue) }
    }

    /// The conversation shown.
    public var selection: Int64? {
        if case let .conversation(threadId) = detail { threadId } else { nil }
    }

    /// The detail shows a composer with a "To:" line: the new message, or messages sent to new recipients.
    public var isComposing: Bool {
        switch detail {
        case .newMessage, .unsent: true
        case .conversation, nil: false
        }
    }

    public var selectedConversation: Conversation? {
        selection.flatMap(conversation)
    }

    /// The "To:" line's recipients.
    public var recipients: [MessagesRecipient] {
        switch detail {
        case .newMessage: newRecipients
        case let .unsent(people): people
        case .conversation, nil: []
        }
    }

    /// The recipients are fixed: the composer sends to those messages were already sent to.
    public var recipientsLocked: Bool {
        if case .unsent = detail { true } else { false }
    }

    /// How many recipients a new message may have: one when the default SIM sends no group messages,
    /// else its limit; `nil` is unlimited.
    public var recipientCap: Int? {
        guard let limits = newMessageLimits else { return nil }
        if !limits.groupAllowed { return 1 }
        return limits.maxRecipients.map(Int.init)
    }

    /// The "To:" field takes another recipient.
    public var canAddRecipient: Bool {
        detail == .newMessage && recipientCap.map { newRecipients.count < $0 } ?? true
    }

    public var composerTarget: MessagesComposerTarget? {
        switch detail {
        case nil: nil
        case let .conversation(threadId): .conversation(threadId)
        case .newMessage: .newMessage
        case let .unsent(people): .recipients(people.map(\.address))
        }
    }

    /// The draft of the shown composer.
    public var draft: MessagesDraft {
        composerTarget.flatMap { drafts[$0] } ?? MessagesDraft()
    }

    public var composer: MessagesComposerState {
        switch detail {
        case nil:
            return MessagesComposerState(isEnabled: false, placeholder: String(localized: "Text Message"), canAttach: false, maxTextLength: nil)
        case .newMessage, .unsent:
            if let cap = recipientCap, recipients.count > cap {
                return MessagesComposerState(
                    isEnabled: false,
                    placeholder: Self.capNotice(cap),
                    canAttach: false,
                    maxTextLength: nil
                )
            }

            // A new conversation sends from the default SIM (composition rule 4).
            let canAttach = newMessageLimits?.attachmentsAllowed ?? state?.settings.sendMms ?? true
            return MessagesComposerState(
                isEnabled: true,
                placeholder: String(localized: "Text Message"),
                canAttach: canAttach,
                maxTextLength: newMessageLimits?.maxTextLength.map(Int.init)
            )
        case let .conversation(threadId):
            guard let conversation = self.conversation(threadId) else {
                return MessagesComposerState(isEnabled: false, placeholder: String(localized: "Text Message"), canAttach: false, maxTextLength: nil)
            }
            guard conversation.canReply else {
                return MessagesComposerState(
                    isEnabled: false,
                    placeholder: String(localized: "You can't reply to this conversation"),
                    canAttach: false,
                    maxTextLength: nil
                )
            }
            // RCS bots do not accept replies.
            let isBot = conversation.recipients.contains { recipient in
                let addr = recipient.address.lowercased()
                return addr.contains("bot-rcs.google.com") || addr.contains("bot.rcs.google.com") || addr.contains("@bot")
            }
            if isBot {
                return MessagesComposerState(
                    isEnabled: false,
                    placeholder: String(localized: "Sender doesn't accept replies"),
                    canAttach: false,
                    maxTextLength: nil
                )
            }
            return MessagesComposerState(
                isEnabled: true,
                placeholder: String(localized: "Text Message"),
                canAttach: conversation.attachmentsAllowed,
                maxTextLength: conversation.maxTextLength.map(Int.init)
            )
        }
    }

    /// The "+" button: images are offered here and the draft has room for one more.
    public var canAddAttachments: Bool {
        let composer = composer
        return composer.isEnabled && composer.canAttach && draft.attachments.count < Self.maxAttachments
    }

    /// Return sends: the draft has something to send and somewhere to send it.
    public var canSubmit: Bool {
        guard composer.isEnabled, draft.hasContent else { return false }
        switch detail {
        case .conversation, .unsent: return true
        case .newMessage: return !newRecipients.isEmpty || !trimmed(recipientQuery).isEmpty
        case nil: return false
        }
    }

    /// Parts whose download runs: requested, or accepted by the phone with their bytes still to come.
    public var downloading: Set<MessagePartRef> {
        requestedDownloads.union(awaitedDownloads.keys)
    }

    /// Why a phone that sends only over Bluetooth cannot send now, for the line above the composer.
    public var bluetoothNotice: String? {
        composerTarget == nil ? nil : MessagesFormat.bluetoothNotice(state?.proximity)
    }

    /// The SIM picker: only for a phone with two SIMs or more, in a conversation (a new message sends
    /// from the default one).
    public var showsSimPicker: Bool {
        selectedConversation != nil && subscriptions.count > 1
    }

    /// The SIM the shown conversation sends from.
    public var activeSubscription: SimSubscription? {
        guard let conversation = selectedConversation else { return nil }
        return subscriptions.first { $0.id == conversation.activeSubscription }
    }

    /// What the new-message composer offers when it cannot search the phone's contacts; `nil` when it
    /// can, or when the phone cannot sync contacts at all.
    public var contactsNotice: MessagesContactsNotice? {
        guard let state = contactsState, state.capable else { return nil }
        switch state.access {
        case .granted: return nil
        case .pcConsentRequired: return .turnOn
        case .deviceConsentRequired, .deviceConsentDenied: return .allowOnPhone
        }
    }

    private var gate: MessagesListStatus? {
        guard let state else { return nil }
        if !state.settings.pcConsent { return .turnedOff }
        if !state.phonePermission { return .permissionNeeded }
        return nil
    }

    private var searchQuery: String {
        trimmed(searchText)
    }

    private func conversation(_ threadId: Int64) -> Conversation? {
        conversations.first { $0.threadId == threadId }
    }

    /// The conversation of exactly these recipients (by `MessagesAddress.sameRecipients`).
    private func conversation(matching addresses: [String]) -> Conversation? {
        conversations.first { MessagesAddress.sameRecipients($0.recipients.map(\.address), addresses) }
    }

    private func unsentGroup(_ addresses: [String]) -> MessagesUnsentConversation? {
        unsent.first { MessagesAddress.sameRecipients($0.addresses, addresses) }
    }

    private static func matches(_ conversation: Conversation, _ query: String) -> Bool {
        conversation.recipients.contains { matches($0, query) }
            || MessagesFormat.preview(conversation).localizedStandardContains(query)
    }

    private static func matches(_ contact: MessageContact, _ query: String) -> Bool {
        contact.address.localizedStandardContains(query) || contact.name?.localizedStandardContains(query) == true
    }

    private static func capNotice(_ cap: Int) -> String {
        cap == 1 ? String(localized: "This SIM sends to one person at a time") : String(localized: "This SIM sends to at most \(cap) people")
    }

    private func trimmed(_ text: String) -> String {
        text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    // MARK: - Selection and the mark-as-read triggers

    /// The user selected a conversation (`nil`: none). The new message keeps what it holds.
    public func select(_ threadId: Int64?) {
        setDetail(threadId.map(MessagesDetail.conversation))
    }

    /// The user selected a row of the list (`nil`: none).
    public func selectItem(_ item: MessagesListItem?) {
        switch item {
        case nil: select(nil)
        case let .conversation(threadId): select(threadId)
        case .newMessage: startNewMessage()
        case let .unsent(addresses): openUnsent(addresses)
        }
    }

    /// The Messages section appeared in the window (`true`) or left it.
    public func setMessagesViewActive(_ active: Bool) {
        messagesViewActive = active
        call("messages view change not passed on") { try api.messagesViewChanged(active: active) }
    }

    /// The window became key (`true`) or stopped being key.
    public func setWindowFocused(_ focused: Bool) {
        windowFocused = focused
        call("window focus not passed on") { try api.windowFocused(focused: focused) }

        // The engine reads the Mac's Bluetooth state when asked: refresh it when the user looks again.
        if focused {
            reloadState()
        }
    }

    private func setDetail(_ new: MessagesDetail?) {
        guard new != detail else { return }
        detail = new
        if new != .newMessage {
            cancelSuggestions()
        }
        reportSelection()
        reloadTranscript()
    }

    /// Tells the engine which conversation the window shows: none while the list and the detail stand
    /// aside for the gate, so a hidden conversation is never marked read; the selection again once the
    /// gate lifts. `replaced` reports a new message's conversation taking the composer's place.
    private func reportSelection(replaced: Bool = false) {
        let shown = isGated ? nil : selection
        if replaced, let shown {
            if call("composed conversation replacement not passed on", { try api.composedConversationReplaced(threadId: shown) }) != nil {
                reportedSelection = .some(shown)
            }
            return
        }

        guard reportedSelection != .some(shown) else { return }
        if call("conversation selection not passed on", { try api.conversationSelected(threadId: shown) }) != nil {
            reportedSelection = .some(shown)
        }
    }

    // MARK: - New message

    /// Shows the new message, as far as it was written.
    public func startNewMessage() {
        guard detail != .newMessage else { return }
        beforeNewMessage = selection
        setDetail(.newMessage)
        reloadContacts()
        scheduleSuggestions()
    }

    /// Escape in the new message: an empty one closes, and the conversation shown before it comes back.
    /// `false` when it holds something (it stays).
    @discardableResult
    public func cancelNewMessage() -> Bool {
        guard detail == .newMessage, !newMessageHasContent else { return false }
        setDetail(shownBeforeNewMessage)
        return true
    }

    /// Deletes the new message being written: its recipients, what is typed in "To:" and its draft.
    public func discardNewMessage() {
        newRecipients = []
        recipientQuery = ""
        drafts[.newMessage] = nil
        cancelSuggestions()
        if detail == .newMessage {
            setDetail(shownBeforeNewMessage)
        }
    }

    /// The conversation shown before the new message opened, when it is still there.
    private var shownBeforeNewMessage: MessagesDetail? {
        guard let threadId = beforeNewMessage, conversation(threadId) != nil else { return nil }
        return .conversation(threadId)
    }

    /// Opens what was sent to new recipients: their messages, and a composer to the same recipients.
    public func openUnsent(_ addresses: [String]) {
        if case let .unsent(people) = detail, MessagesAddress.sameRecipients(people.map(\.address), addresses) { return }
        guard let group = unsentGroup(addresses) else { return }
        setDetail(.unsent(group.recipients.map { MessagesRecipient(address: $0.address, name: $0.name) }))
    }

    /// Writes to one person (a contact's Message button): turns messages on when they are off, then
    /// shows the conversation whose only recipient they are, else the new message addressed to them
    /// alone, keeping what its draft holds.
    public func write(to recipient: MessagesRecipient) {
        if state?.settings.pcConsent == false {
            turnOnMessages()
        }

        if let found = conversation(matching: [recipient.address]) {
            select(found.threadId)
            return
        }

        newRecipients = [recipient]
        recipientQuery = ""
        cancelSuggestions()
        startNewMessage()
    }

    public func setRecipientQuery(_ query: String) {
        guard detail == .newMessage else { return }
        recipientQuery = query
        scheduleSuggestions()
    }

    /// Return in the "To:" field: the first suggestion, else what was typed when it is a number or an
    /// email address.
    public func submitRecipientQuery() {
        if let first = suggestions.first {
            addRecipient(first)
            return
        }

        let typed = trimmed(recipientQuery)
        if MessagesAddress.isPlausible(typed) {
            appendRecipient(MessagesRecipient(address: typed, name: nil))
        }
    }

    public func addRecipient(_ suggestion: ContactSuggestion) {
        appendRecipient(MessagesRecipient(address: suggestion.number, name: suggestion.name))
    }

    public func removeRecipient(_ recipient: MessagesRecipient) {
        guard detail == .newMessage else { return }
        newRecipients.removeAll { $0 == recipient }
        scheduleSuggestions()
    }

    /// Asks the phone to allow contacts, which the suggestions come from.
    public func requestContactsPermission() async {
        guard !isRequestingContacts else { return }
        isRequestingContacts = true
        defer { isRequestingContacts = false }

        do {
            let result = try await contacts.requestPermission()
            let failure = result.timedOut ? String(localized: "Your phone didn't respond.") : UserText.failure(result.outcome)
            if let failure {
                log.error("contacts permission not granted: \(result.outcome, privacy: .public), timed out \(result.timedOut)")
                alert = MessagesAlert(title: String(localized: "Contacts Not Allowed"), message: failure)
            } else if let sync = result.sync, UserText.failure(sync) != nil {
                log.notice("contacts not synced after the grant: \(sync, privacy: .public)")
            }

            reloadContacts()
        } catch {
            log.error("contacts permission not requested: \(error: error)")
            alert = MessagesAlert(title: String(localized: "Contacts Not Allowed"), message: UserText.describe(error))
        }
    }

    /// Turns on "Use my phone's contacts".
    public func turnOnContacts() {
        do {
            try contacts.setPcConsent(consent: true)
            reloadContacts()
        } catch {
            log.error("contacts not turned on: \(error: error)")
            alert = MessagesAlert(title: String(localized: "Contacts Not Turned On"), message: UserText.describe(error))
        }
    }

    private func appendRecipient(_ recipient: MessagesRecipient) {
        guard detail == .newMessage, canAddRecipient else { return }
        if !newRecipients.contains(where: { MessagesAddress.same($0.address, recipient.address) }) {
            newRecipients.append(recipient)
        }

        recipientQuery = ""
        cancelSuggestions()
    }

    private func cancelSuggestions() {
        suggestionSearch?.cancel()
        suggestionSearch = nil
        generations.suggestions += 1
        suggestions = []
    }

    /// Searches the contacts for what is typed in "To:" once typing pauses — only while the
    /// contacts may be used and another recipient fits.
    private func scheduleSuggestions() {
        let query = trimmed(recipientQuery)
        guard detail == .newMessage, !query.isEmpty, contactsState?.access == .granted, canAddRecipient else {
            cancelSuggestions()
            return
        }

        suggestionSearch?.cancel()
        generations.suggestions += 1
        let generation = generations.suggestions
        guard timing.searchDelay > .zero else {
            readSuggestions(query, generation: generation)
            return
        }

        let delay = timing.searchDelay
        suggestionSearch = Task { [weak self] in
            try? await Task.sleep(for: delay)
            guard !Task.isCancelled else { return }
            self?.readSuggestions(query, generation: generation)
        }
    }

    private func readSuggestions(_ query: String, generation: Int) {
        let limit = Self.suggestionLimit
        reads.read({ [contacts] in try contacts.suggestions(query: query, limit: limit) }) { [weak self] result in
            guard let self, generation == self.generations.suggestions else { return }
            switch result {
            case let .success(found):
                suggestions = found.filter { suggestion in
                    !newRecipients.contains { MessagesAddress.same($0.address, suggestion.number) }
                }
            case let .failure(error):
                record(error, "contact suggestions not read")
            }
        }
    }

    /// Messages sent to new recipients move to the conversation the phone created for them (or the one
    /// those recipients already had), with what is typed meanwhile; writing to a hidden conversation
    /// shows it again. `true` when they moved.
    @discardableResult
    private func adoptComposedConversation() -> Bool {
        guard case let .unsent(people) = detail else { return false }
        let addresses = people.map(\.address)
        guard let found = conversation(matching: addresses) else { return false }

        let composed = MessagesComposerTarget.recipients(addresses)
        let destination = MessagesComposerTarget.conversation(found.threadId)
        if let leftover = drafts[composed] {
            // What is being typed stays as it is; a draft the conversation had follows it.
            drafts[destination] = Self.joined(leftover, drafts[destination])
            drafts[composed] = nil
        }

        detail = .conversation(found.threadId)
        // The engine takes this as the selection and marks the conversation read when it is seen.
        reportSelection(replaced: true)
        if found.hidden {
            setFlags(found.threadId, muted: found.muted, hidden: false)
        }
        reloadTranscript()
        return true
    }

    // MARK: - Drafts and sending

    public func setDraftText(_ text: String) {
        guard let target = composerTarget else { return }
        var draft = drafts[target] ?? MessagesDraft()
        draft.text = MessagesTextLimit.clamp(text, maxUTF16: composer.maxTextLength)
        store(draft, for: target)
    }

    /// Adds images to the shown draft: images only, at most `maxAttachments` per message. `false` when
    /// none was added.
    @discardableResult
    public func addAttachments(_ items: [MessagesAttachment]) -> Bool {
        guard let target = composerTarget, !items.isEmpty else { return false }
        guard composer.isEnabled, composer.canAttach else {
            alert = MessagesAlert(title: String(localized: "Images Not Added"), message: String(localized: "This conversation can't take attachments."))
            return false
        }

        var draft = drafts[target] ?? MessagesDraft()
        let images = items.filter(\.isImage)
        let room = max(Self.maxAttachments - draft.attachments.count, 0)
        let added = images.prefix(room)
        draft.attachments += added
        store(draft, for: target)

        if images.count < items.count {
            alert = MessagesAlert(title: String(localized: "Images Not Added"), message: String(localized: "Only images can be added to a message."))
        } else if added.count < images.count {
            alert = MessagesAlert(title: String(localized: "Images Not Added"), message: String(localized: "A message can have up to \(Self.maxAttachments) images."))
        }
        return !added.isEmpty
    }

    public func removeAttachment(_ id: UUID) {
        guard let target = composerTarget, var draft = drafts[target] else { return }
        draft.attachments.removeAll { $0.id == id }
        store(draft, for: target)
    }

    /// Sends the shown draft. The draft clears at once — the message shows in the transcript as it is
    /// sent — and comes back, with the reason in an alert, when the engine refuses it. A new message
    /// takes a number still in the "To:" field, and its recipients lock now: the composer then shows
    /// what was sent to them until the phone creates their conversation, which takes its place.
    public func send() async {
        guard let target = composerTarget, canSubmit else { return }

        let destination: MessageTarget
        var sentTo: [MessagesRecipient]?
        switch detail {
        case let .conversation(threadId):
            destination = .conversation(threadId: threadId)
        case let .unsent(people):
            destination = .newConversation(recipients: people.map(\.address))
        case .newMessage:
            var people = newRecipients
            let typed = trimmed(recipientQuery)
            if !typed.isEmpty {
                guard MessagesAddress.isPlausible(typed) else {
                    alert = MessagesAlert(title: String(localized: "Message Not Sent"), message: String(localized: "“\(typed)” isn't a phone number or an email address."))
                    return
                }
                if !people.contains(where: { MessagesAddress.same($0.address, typed) }) {
                    people.append(MessagesRecipient(address: typed, name: nil))
                }
            }

            if let cap = recipientCap, people.count > cap {
                alert = MessagesAlert(title: String(localized: "Message Not Sent"), message: Self.capNotice(cap) + ".")
                return
            }
            destination = .newConversation(recipients: people.map(\.address))
            sentTo = people
        case nil:
            return
        }

        let draft = drafts[target] ?? MessagesDraft()
        drafts[target] = nil
        if let sentTo {
            newRecipients = []
            recipientQuery = ""
            cancelSuggestions()
            setDetail(.unsent(sentTo))
        }
        if case let .conversation(threadId) = target, let conversation = conversation(threadId), conversation.hidden {
            setFlags(threadId, muted: conversation.muted, hidden: false)
        }

        do {
            _ = try await api.send(
                target: destination,
                text: draft.text,
                attachments: draft.attachments.map(\.engineAttachment)
            )
            reloadSnapshot()
            reloadTranscript()
        } catch {
            restore(draft, of: target, sentTo: sentTo)
            log.error("message not sent: \(error: error)")
            alert = MessagesAlert(title: String(localized: "Message Not Sent"), message: UserText.describe(error))
        }
    }

    /// A refused send's draft goes back where it was typed, before what was typed there meanwhile. A
    /// refused new message goes back to the composer of earlier messages to the same recipients, else
    /// to the new message with its recipients, editable again — unless another new message was started.
    private func restore(_ draft: MessagesDraft, of target: MessagesComposerTarget, sentTo: [MessagesRecipient]?) {
        guard let sentTo else {
            drafts[target] = Self.joined(draft, drafts[target])
            return
        }

        let addresses = sentTo.map(\.address)
        if unsentGroup(addresses) != nil {
            let composed = MessagesComposerTarget.recipients(addresses)
            drafts[composed] = Self.joined(draft, drafts[composed])
            return
        }

        let reopen = detail == .unsent(sentTo)
        if newRecipients.isEmpty, trimmed(recipientQuery).isEmpty, drafts[.newMessage]?.isEmpty ?? true {
            newRecipients = sentTo
            drafts[.newMessage] = draft
        }
        if reopen {
            setDetail(.newMessage)
        }
    }

    /// Sends a message the phone could not deliver again.
    public func retry(_ sendId: Int64) async {
        guard !retrying.contains(sendId) else { return }
        retrying.insert(sendId)
        defer { retrying.remove(sendId) }

        do {
            _ = try await api.retry(sendId: sendId)
            reloadSnapshot()
            reloadTranscript()
        } catch {
            log.error("message not resent: \(error: error)")
            alert = MessagesAlert(title: String(localized: "Message Not Sent"), message: UserText.describe(error))
        }
    }

    /// Asks the phone for a part's bytes. The part shows as downloading until its bytes arrive with the
    /// conversation's content, or the request fails, or the bytes do not come within the wait —
    /// whichever conversation the window shows meanwhile.
    public func download(_ part: MessagePartRef) async {
        guard !downloading.contains(part) else { return }
        requestedDownloads.insert(part)

        let outcome: RequestOutcome
        do {
            outcome = try await api.downloadPart(part: part)
        } catch {
            requestedDownloads.remove(part)
            log.error("attachment download not requested: \(error: error)")
            alert = MessagesAlert(title: String(localized: "Attachment Not Downloaded"), message: UserText.describe(error))
            return
        }

        requestedDownloads.remove(part)
        if let failure = UserText.failure(outcome) {
            log.error("attachment not downloaded: \(outcome, privacy: .public)")
            alert = MessagesAlert(title: String(localized: "Attachment Not Downloaded"), message: failure)
            return
        }

        guard !arrived(part) else { return }
        downloadWaits += 1
        let wait = downloadWaits
        awaitedDownloads[part] = wait
        let delay = timing.downloadWait
        Task { [weak self] in
            try? await Task.sleep(for: delay)
            self?.downloadWaitEnded(part, wait: wait)
        }
    }

    /// The wait ran out without the shown transcript seeing the bytes. The part's conversation may not
    /// be the one shown, so the engine says whether they came meanwhile; only when they did not is the
    /// download reported as failed.
    private func downloadWaitEnded(_ part: MessagePartRef, wait: Int) {
        guard awaitedDownloads[part] == wait else { return }
        reads.read({ [api] in try api.partBytes(part: part) != nil }) { [weak self] result in
            guard let self, awaitedDownloads[part] == wait else { return }
            awaitedDownloads[part] = nil
            switch result {
            case .success(true):
                return
            case .success(false):
                log.error("attachment not downloaded; its bytes did not arrive within the wait")
            case let .failure(error):
                log.error("downloaded attachment not read: \(error: error)")
            }

            alert = MessagesAlert(title: String(localized: "Attachment Not Downloaded"), message: String(localized: "Your phone didn't send the attachment."))
        }
    }

    /// The transcript shows the part with its bytes (or no longer as downloadable).
    private func arrived(_ part: MessagePartRef) -> Bool {
        for row in transcript {
            if let shown = row.message.parts.first(where: { $0.part == part }) {
                return shown.hasBytes || shown.state != .downloadable
            }
        }
        return false
    }

    private func store(_ draft: MessagesDraft, for target: MessagesComposerTarget) {
        drafts[target] = draft.isEmpty ? nil : draft
    }

    /// `first` with `then` after it: the texts on separate lines, the images as many as fit.
    private static func joined(_ first: MessagesDraft, _ then: MessagesDraft?) -> MessagesDraft {
        guard let then, !then.isEmpty else { return first }
        var draft = MessagesDraft()
        draft.text = [first.text, then.text].filter { !$0.isEmpty }.joined(separator: "\n")
        draft.attachments = Array((first.attachments + then.attachments).prefix(maxAttachments))
        return draft
    }

    // MARK: - Conversations and settings

    /// "Hide Alerts" (`true`) or "Show Alerts".
    public func setMuted(_ muted: Bool, threadId: Int64) {
        guard let conversation = self.conversation(threadId) else { return }
        setFlags(threadId, muted: muted, hidden: conversation.hidden)
    }

    /// Hides a conversation or shows it again, undoable with `undoManager`.
    public func setHidden(_ hidden: Bool, threadId: Int64, undoManager: UndoManager?) {
        guard let conversation = self.conversation(threadId), setFlags(threadId, muted: conversation.muted, hidden: hidden) else { return }
        if hidden && selection == threadId {
            setDetail(nil)
        }

        let undoing = undoManager?.isUndoing == true
        undoManager?.registerUndo(withTarget: self) { model in
            model.setHidden(!hidden, threadId: threadId, undoManager: undoManager)
        }
        // While undoing, the redo is named after the action undone.
        undoManager?.setActionName(hidden != undoing ? String(localized: "Hide Conversation") : String(localized: "Show Conversation"))
    }

    /// The SIM the shown conversation sends from.
    public func selectSubscription(_ subscriptionId: Int64) {
        guard let threadId = selection else { return }
        do {
            try api.selectSubscription(threadId: threadId, subscriptionId: subscriptionId)
            reloadSnapshot()
        } catch {
            log.error("sim not changed: \(error: error)")
            alert = MessagesAlert(title: String(localized: "SIM Not Changed"), message: UserText.describe(error))
        }
    }

    /// Asks the phone to allow messages on this Mac.
    public func requestPermission() async {
        guard !isRequestingPermission else { return }
        isRequestingPermission = true
        defer { isRequestingPermission = false }

        do {
            let outcome = try await api.requestPermission()
            if let failure = UserText.failure(outcome) {
                log.error("messages permission not granted: \(outcome, privacy: .public)")
                alert = MessagesAlert(title: String(localized: "Messages Not Allowed"), message: failure)
            }

            reloadState()
        } catch {
            log.error("messages permission not requested: \(error: error)")
            alert = MessagesAlert(title: String(localized: "Messages Not Allowed"), message: UserText.describe(error))
        }
    }

    /// Turns on "Show messages on this Mac".
    public func turnOnMessages() {
        guard var settings = state?.settings else { return }
        settings.pcConsent = true
        do {
            try api.setSettings(settings: settings)
            reloadState()
            reloadSnapshot()
        } catch {
            log.error("messages not turned on: \(error: error)")
            alert = MessagesAlert(title: String(localized: "Messages Not Turned On"), message: UserText.describe(error))
        }
    }

    @discardableResult
    private func setFlags(_ threadId: Int64, muted: Bool, hidden: Bool) -> Bool {
        do {
            try api.setConversationFlags(threadId: threadId, muted: muted, hidden: hidden)
            reloadSnapshot()
            return true
        } catch {
            log.error("conversation not changed: \(error: error)")
            alert = MessagesAlert(title: String(localized: "Conversation Not Changed"), message: UserText.describe(error))
            return false
        }
    }

    // MARK: - Pictures

    /// A picture decoded at most `maxPixels` on its longer side, read and decoded off the main actor
    /// and kept; `nil` when there is none. A failed read is not kept: the next call reads it again.
    public func image(of picture: MessagesPicture, maxPixels: Int) async -> NSImage? {
        let key: String = switch picture {
        case let .part(part, size): "part-\(part)-\(size)-\(maxPixels)"
        case let .contact(id): "contact-\(photosGeneration)-\(id)-\(maxPixels)"
        }

        if let cached = images.object(forKey: key as NSString) { return cached }
        guard !withoutImage.contains(key) else { return nil }

        let result = await readNow { [api, contacts] () throws -> CGImage? in
            let data = switch picture {
            case let .part(part, _): try api.partBytes(part: part)
            case let .contact(id): try contacts.thumbnail(contactId: id)
            }
            return data.flatMap { PhotosImageDecoder.thumbnail(of: $0, maxPixelSize: maxPixels) }
        }
        return keep(result, key: key)
    }

    /// A draft image's thumbnail, decoded off the main actor and kept.
    public func image(of attachment: MessagesAttachment, maxPixels: Int) async -> NSImage? {
        let key = "draft-\(attachment.id)-\(maxPixels)"
        if let cached = images.object(forKey: key as NSString) { return cached }
        guard !withoutImage.contains(key) else { return nil }
        let data = attachment.data
        let result = await readNow { PhotosImageDecoder.thumbnail(of: data, maxPixelSize: maxPixels) }
        return keep(result, key: key)
    }

    /// A part's bytes, read off the main actor (to open an attachment in its app).
    public func partData(_ part: MessagePartRef) async throws -> Data? {
        try await readNow { [api] in try api.partBytes(part: part) }.get()
    }

    private func keep(_ result: Result<CGImage?, any Error>, key: String) -> NSImage? {
        switch result {
        case let .success(image?):
            // Two pixels a point: the size Retina displays show the thumbnail at.
            let shown = NSImage(cgImage: image, size: NSSize(width: CGFloat(image.width) / 2, height: CGFloat(image.height) / 2))
            images.setObject(shown, forKey: key as NSString)
            return shown
        case .success(nil):
            withoutImage.insert(key)
            return nil
        case let .failure(error):
            // Each cell asks again until its picture reads: the first failure of a run is logged.
            if !Self.notStarted(error), !pictureFailureLogged {
                pictureFailureLogged = true
                log.notice("message picture not read: \(error: error); later failures are not logged until the next start")
            }
            return nil
        }
    }

    private func readNow<T: Sendable>(_ work: @escaping @Sendable () throws -> T) async -> Result<T, any Error> {
        await withCheckedContinuation { continuation in
            reads.read(work) { continuation.resume(returning: $0) }
        }
    }

    // MARK: - Engine events and loading

    private func handle(_ event: EngineEvent) {
        switch event {
        case let .messaging(change):
            handle(change)
        case let .contacts(change):
            switch change {
            case .recordsChanged: contactsChanged()
            case .stateChanged: reloadContacts()
            }
        case .resync(.messaging):
            reload()
        case .resync(.contacts):
            reloadContacts()
            contactsChanged()
        default:
            break
        }
    }

    private func handle(_ change: MessagingChange) {
        switch change {
        case let .contentChanged(threads):
            // Adoption is tried against every conversation when they arrive.
            reloadSnapshot()
            if threads.isEmpty || selection.map(threads.contains) == true {
                reloadTranscript()
            }
        case .sendChanged:
            reloadSnapshot()
            if selection != nil {
                reloadTranscript()
            }
        case .stateChanged:
            // The settings decide which conversations take attachments.
            reloadState()
            reloadSnapshot()
        case .permissionChanged:
            reloadState()
        case let .openConversation(threadId):
            navigator.showConversation(threadId)
        }
    }

    private func engineStarted() {
        pictureFailureLogged = false
        // A new engine core knows nothing of the window.
        reportedSelection = .some(nil)

        if let messagesViewActive {
            call("messages view change not passed on") { try api.messagesViewChanged(active: messagesViewActive) }
        }
        if let windowFocused {
            call("window focus not passed on") { try api.windowFocused(focused: windowFocused) }
        }

        reload()
        reportSelection()
    }

    /// Names in conversations and messages come from contacts, as do the suggestions and the photos.
    private func contactsChanged() {
        photosGeneration += 1
        withoutImage = withoutImage.filter { !$0.hasPrefix("contact-") }
        reloadSnapshot()
        reloadTranscript()
        scheduleSuggestions()
    }

    private func reload() {
        reloadState()
        reloadSnapshot()
        reloadContacts()
        reloadTranscript()
    }

    private func reloadState() {
        generations.state += 1
        let generation = generations.state

        reads.read({ [api] in try api.state() }) { [weak self] result in
            guard let self, generation == self.generations.state else { return }
            switch result {
            case let .success(loaded):
                state = loaded
                // The gate may have closed or lifted.
                reportSelection()
            case let .failure(error):
                record(error, "messaging state not read")
            }
        }
    }

    private func reloadContacts() {
        generations.contacts += 1
        let generation = generations.contacts

        reads.read({ [contacts] in try contacts.state() }) { [weak self] result in
            guard let self, generation == self.generations.contacts else { return }
            switch result {
            case let .success(loaded):
                let changed = contactsState?.access != loaded.access
                contactsState = loaded
                if changed {
                    scheduleSuggestions()
                }
            case let .failure(error):
                record(error, "contacts state not read")
            }
        }
    }

    /// The conversations, the SIMs, what was sent to new recipients and the new message's limits.
    private func reloadSnapshot() {
        generations.snapshot += 1
        let generation = generations.snapshot

        reads.read({ [api] in
            MessagesSnapshot(
                conversations: try api.conversations(),
                subscriptions: try api.subscriptions(),
                unsent: try api.newConversationMessages(),
                limits: try api.newMessageLimits()
            )
        }) { [weak self] result in
            guard let self, generation == self.generations.snapshot else { return }
            switch result {
            case let .success(snapshot):
                apply(snapshot)
            case let .failure(error):
                record(error, "conversations not read")
            }
        }
    }

    private func apply(_ snapshot: MessagesSnapshot) {
        conversations = snapshot.conversations
        subscriptions = snapshot.subscriptions
        unsent = MessagesUnsent.groups(snapshot.unsent)
        newMessageLimits = snapshot.limits

        if let selection, conversation(selection) == nil {
            setDetail(nil)
        }

        if !adoptComposedConversation() {
            // What was sent to new recipients shows in their composer or their conversation.
            rebuildTranscript()
        }

        if detail == .newMessage, !canAddRecipient {
            cancelSuggestions()
        }
    }

    /// Reads the shown conversation's messages again (also the detail's Try Again).
    public func reloadTranscript() {
        generations.transcript += 1
        let generation = generations.transcript

        let owner = detail
        if owner != transcriptOwner {
            transcriptOwner = owner
            transcriptMessages = []
            transcriptLoaded = false
            transcriptFailed = false
        }

        guard case let .conversation(threadId) = owner else {
            // A composer's messages come with the conversations (what was sent to new recipients).
            rebuildTranscript()
            return
        }
        if transcriptMessages.isEmpty {
            rebuildTranscript()
        }

        reads.read({ [api] in try api.messages(threadId: threadId) }) { [weak self] result in
            guard let self, generation == self.generations.transcript, detail == owner else { return }
            switch result {
            case let .success(messages):
                transcriptMessages = messages
                transcriptLoaded = true
                transcriptFailed = false
            case let .failure(error):
                record(error, "messages not read")
                // Before the engine starts there is nothing to show yet, which is no failure.
                transcriptFailed = !transcriptLoaded && !Self.notStarted(error)
            }

            rebuildTranscript()
        }
    }

    /// The transcript of the shown detail: a conversation's messages, then its sends — those of its
    /// composer and those to its recipients from a new message — in the order they were sent.
    private func rebuildTranscript() {
        let messages: [Message]
        let isGroup: Bool
        switch detail {
        case nil, .newMessage:
            messages = []
            isGroup = newRecipients.count > 1
        case let .unsent(people):
            messages = unsentGroup(people.map(\.address))?.messages ?? []
            isGroup = people.count > 1
        case let .conversation(threadId):
            let conversation = conversation(threadId)
            let merged = conversation.map { c in
                unsent.filter { MessagesAddress.sameRecipients($0.addresses, c.recipients.map(\.address)) }.flatMap(\.messages)
            } ?? []
            let synced = transcriptMessages.filter { $0.kind != .pendingSend }
            let sends = (transcriptMessages.filter { $0.kind == .pendingSend } + merged).sorted { $0.id < $1.id }
            messages = synced + sends
            isGroup = conversation?.isGroup ?? false
        }

        transcript = MessagesTranscript.rows(messages, isGroup: isGroup, time: time)
        for part in awaitedDownloads.keys where arrived(part) {
            awaitedDownloads[part] = nil
        }
    }

    /// Runs a local engine call; `nil` when it failed. Before the engine starts every call fails with
    /// `NotStarted`, which only means there is nothing to show yet; other failures are logged.
    @discardableResult
    private func call<T>(_ subject: StaticString, _ body: () throws -> T) -> T? {
        do {
            return try body()
        } catch {
            if !Self.notStarted(error) {
                log.notice("\(subject.description, privacy: .public): \(error: error)")
            }
            return nil
        }
    }

    /// A read that failed: the last data stays shown.
    private func record(_ error: any Error, _ subject: StaticString) {
        guard !Self.notStarted(error) else { return }
        log.error("\(subject.description, privacy: .public): \(error: error)")
    }

    private static func notStarted(_ error: any Error) -> Bool {
        if case FeatureApiError.NotStarted? = error as? FeatureApiError { true } else { false }
    }
}

/// The list-level data one read brings.
private struct MessagesSnapshot: Sendable {
    let conversations: [Conversation]
    let subscriptions: [SimSubscription]
    let unsent: [NewConversationMessage]
    let limits: NewMessageLimits
}

/// The newest read of each kind; an older one's result is dropped.
private struct ReadGenerations {
    var state = 0
    var contacts = 0
    var snapshot = 0
    var transcript = 0
    var suggestions = 0
}
