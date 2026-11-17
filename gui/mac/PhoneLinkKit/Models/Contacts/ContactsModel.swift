import AppKit
import Observation
import os
import PhoneLinkEngine

private let log = AppLog(category: "contacts")

/// What the Contacts section shows in place of, or as, its list.
public enum ContactsContent: Equatable, Sendable {
    /// The engine has not started.
    case nothing
    /// "Use my phone's contacts" is off on this Mac.
    case turnedOff
    /// The phone has not allowed contacts yet (`denied`: it denied them after they were set up once).
    case needsPhoneAccess(denied: Bool)
    /// The phone is sending its contacts and none are here yet.
    case syncing
    case empty
    case list
}

/// The section's waits.
struct ContactsTiming: Sendable {
    /// After the last keystroke in the search field, the list filters after this long.
    var searchDelay: Duration
    /// Contacts that are not up to date show as syncing for at most this long.
    var syncWait: Duration

    static let live = ContactsTiming(searchDelay: .milliseconds(300), syncWait: .seconds(120))
}

/// The Contacts section: the phone's contacts, grouped and searched as `ContactsIndex` says, the
/// selected contact's card, the prompt that stands in for them while contacts may not be used, and
/// the actions of a card's fields. The view sends nothing to the phone when it opens; it asks the
/// phone only when the user acts on the prompt.
///
/// Engine reads run off the main actor; a result a newer read of the same kind superseded is dropped,
/// and a failed read is logged while the last data stays. Requests the user started report their
/// failures as `alert`.
@MainActor
@Observable
public final class ContactsModel {
    /// `nil` until the engine has started.
    public private(set) var state: ContactsState?
    /// Every contact, in the engine's order.
    public private(set) var contacts: [ContactListItem] = []
    /// The list's groups, filtered by the search once typing paused.
    public private(set) var sections: [ContactsSection] = []
    /// The search field's text; the list follows it after `ContactsTiming.searchDelay`.
    public var searchText = "" {
        didSet { scheduleFilter() }
    }
    /// The selected contact's id.
    public var selection: Int64? {
        didSet {
            if selection != oldValue { reloadCard() }
        }
    }
    /// The selected contact's card; `nil` while it is read or when it is gone.
    public private(set) var card: ContactCard?
    public private(set) var isRequestingPermission = false
    /// The phone did not answer the last permission request.
    public private(set) var permissionTimedOut = false
    /// The wait for contacts to come up to date ran out.
    public private(set) var syncWaitOver = false
    /// The failure of an action the user started.
    public var alert: MessagesAlert?

    @ObservationIgnored private var appliedQuery = ""
    @ObservationIgnored private var filterTask: Task<Void, Never>?
    @ObservationIgnored private var syncWaitTask: Task<Void, Never>?
    @ObservationIgnored private var generations = ContactsGenerations()
    @ObservationIgnored private var withoutImage: Set<String> = []
    /// Bumped when contacts change, so their photos are read again.
    @ObservationIgnored private var photosGeneration = 0
    @ObservationIgnored private var pictureFailureLogged = false
    private let images = NSCache<NSString, NSImage>()

    private let api: any ContactsApiProtocol
    private let navigator: AppNavigator
    private let reads: any MessagesReading
    private let timing: ContactsTiming
    /// Opens a URL in its app (injected so tests never launch one).
    private let openURL: @MainActor (URL) -> Void

    public convenience init(api: any ContactsApiProtocol, hub: EngineEventHub, navigator: AppNavigator) {
        self.init(
            api: api,
            hub: hub,
            navigator: navigator,
            reads: MessagesBackgroundReads(),
            timing: .live,
            openURL: { NSWorkspace.shared.open($0) }
        )
    }

    init(
        api: any ContactsApiProtocol,
        hub: EngineEventHub,
        navigator: AppNavigator,
        reads: any MessagesReading,
        timing: ContactsTiming,
        openURL: @escaping @MainActor (URL) -> Void
    ) {
        self.api = api
        self.navigator = navigator
        self.reads = reads
        self.timing = timing
        self.openURL = openURL

        hub.onEvent { [weak self] event in
            self?.handle(event)
        }
        hub.onStart { [weak self] in
            self?.engineStarted()
        }
    }

    // MARK: - What the views show

    /// The sidebar lists Contacts only for a phone that syncs its contacts.
    public var isViewVisible: Bool {
        state?.capable == true
    }

    public var content: ContactsContent {
        guard let state else { return .nothing }
        switch state.access {
        case .pcConsentRequired: return .turnedOff
        case .deviceConsentRequired: return .needsPhoneAccess(denied: false)
        case .deviceConsentDenied: return .needsPhoneAccess(denied: true)
        case .granted: break
        }

        if !contacts.isEmpty { return .list }
        return isSyncing ? .syncing : .empty
    }

    /// Contacts are not up to date, and the wait for them has not run out.
    public var isSyncing: Bool {
        guard let state, state.access == .granted else { return false }
        return !state.upToDate && !syncWaitOver
    }

    /// The search filtered every contact out.
    public var searchFoundNothing: Bool {
        !appliedQuery.isEmpty && sections.isEmpty && !contacts.isEmpty
    }

    /// A contact's name as the list shows it: its shown name, or that it has none.
    public static func name(_ name: String) -> String {
        name.isEmpty ? String(localized: "No Name") : name
    }

    // MARK: - The prompt

    /// Turns on "Use my phone's contacts".
    public func turnOn() {
        do {
            try api.setPcConsent(consent: true)
            reloadState()
        } catch {
            log.error("contacts not turned on: \(error: error)")
            alert = MessagesAlert(title: String(localized: "Contacts Not Turned On"), message: UserText.describe(error))
        }
    }

    /// Asks the phone to allow contacts; a grant is followed by a contacts sync.
    public func requestPermission() async {
        guard !isRequestingPermission else { return }
        isRequestingPermission = true
        permissionTimedOut = false
        defer { isRequestingPermission = false }

        do {
            let result = try await api.requestPermission()
            if result.timedOut {
                log.notice("contacts permission request timed out")
                permissionTimedOut = true
            } else if let failure = UserText.failure(result.outcome) {
                log.error("contacts permission not granted: \(result.outcome, privacy: .public)")
                alert = MessagesAlert(title: String(localized: "Contacts Not Allowed"), message: failure)
            } else if let sync = result.sync, UserText.failure(sync) != nil {
                log.notice("contacts not synced after the grant: \(sync, privacy: .public)")
            }

            reloadState()
        } catch {
            log.error("contacts permission not requested: \(error: error)")
            alert = MessagesAlert(title: String(localized: "Contacts Not Allowed"), message: UserText.describe(error))
        }
    }

    // MARK: - Field actions

    /// Messages, addressed to this number: its conversation, else a new message.
    public func message(_ number: ContactCardNumber) {
        let name = card?.name ?? ""
        navigator.showMessage(to: MessagesRecipient(address: number.number, name: name.isEmpty ? nil : name))
    }

    public func email(_ address: String) {
        open(URL(string: "mailto:" + Self.escaped(address)), "email address")
    }

    /// Shows the address in Maps.
    public func showOnMap(_ address: String) {
        open(URL(string: "maps://?q=" + Self.escaped(address)), "postal address")
    }

    /// Opens a contact's URL: as given when it has a scheme, else as `http://`.
    public func open(link: String) {
        open(Self.url(of: link), "URL")
    }

    private func open(_ url: URL?, _ subject: StaticString) {
        guard let url else {
            log.notice("contact \(subject.description, privacy: .public) not opened: it makes no URL")
            return
        }
        openURL(url)
    }

    static func url(of link: String) -> URL? {
        let trimmed = link.trimmingCharacters(in: .whitespacesAndNewlines)
        if let url = URL(string: trimmed), url.scheme?.isEmpty == false {
            return url
        }
        return URL(string: "http://" + trimmed)
    }

    /// Every character but the unreserved ASCII ones (`A`–`Z`, `a`–`z`, `0`–`9`, `-._~`) percent-encoded.
    static func escaped(_ text: String) -> String {
        text.addingPercentEncoding(withAllowedCharacters: unreserved) ?? ""
    }

    private static let unreserved = CharacterSet(charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~")

    // MARK: - Pictures

    /// A contact's photo decoded at most `maxPixels` on its longer side, read off the main actor and
    /// kept; `nil` when it has none.
    public func image(of contactId: Int64, maxPixels: Int) async -> NSImage? {
        let key = "\(photosGeneration)-\(contactId)-\(maxPixels)"
        if let cached = images.object(forKey: key as NSString) { return cached }
        guard !withoutImage.contains(key) else { return nil }

        let result = await withCheckedContinuation { continuation in
            reads.read({ [api] () throws -> CGImage? in
                try api.thumbnail(contactId: contactId).flatMap { PhotosImageDecoder.thumbnail(of: $0, maxPixelSize: maxPixels) }
            }) { continuation.resume(returning: $0) }
        }

        switch result {
        case let .success(image?):
            let shown = NSImage(cgImage: image, size: NSSize(width: CGFloat(image.width) / 2, height: CGFloat(image.height) / 2))
            images.setObject(shown, forKey: key as NSString)
            return shown
        case .success(nil):
            withoutImage.insert(key)
            return nil
        case let .failure(error):
            if !Self.notStarted(error), !pictureFailureLogged {
                pictureFailureLogged = true
                log.notice("contact photo not read: \(error: error); later failures are not logged until the next start")
            }
            return nil
        }
    }

    // MARK: - Engine events and loading

    private func handle(_ event: EngineEvent) {
        switch event {
        case let .contacts(change):
            switch change {
            case .recordsChanged: contactsChanged()
            case .stateChanged: reloadState()
            }
        case .resync(.contacts):
            reloadState()
            contactsChanged()
        default:
            break
        }
    }

    private func engineStarted() {
        pictureFailureLogged = false
        syncWaitTask?.cancel()
        syncWaitTask = nil
        syncWaitOver = false
        permissionTimedOut = false

        reloadState()
        contactsChanged()
    }

    private func contactsChanged() {
        photosGeneration += 1
        withoutImage = []
        reloadList()
        reloadCard()
    }

    private func reloadState() {
        generations.state += 1
        let generation = generations.state

        reads.read({ [api] in try api.state() }) { [weak self] result in
            guard let self, generation == self.generations.state else { return }
            switch result {
            case let .success(loaded):
                let accessChanged = state != nil && state?.access != loaded.access
                state = loaded
                waitForUpToDate()

                // The list and the card are empty while the prompt stands in for them.
                if accessChanged {
                    reloadList()
                    reloadCard()
                }
            case let .failure(error):
                record(error, "contacts state not read")
            }
        }
    }

    /// Starts the wait for contacts to come up to date when they are not, once per start.
    private func waitForUpToDate() {
        guard let state, !state.upToDate, syncWaitTask == nil, !syncWaitOver else { return }
        let wait = timing.syncWait
        syncWaitTask = Task { [weak self] in
            try? await Task.sleep(for: wait)
            guard !Task.isCancelled else { return }
            self?.syncWaitOver = true
        }
    }

    private func reloadList() {
        generations.list += 1
        let generation = generations.list

        reads.read({ [api] in try api.list() }) { [weak self] result in
            guard let self, generation == self.generations.list else { return }
            switch result {
            case let .success(loaded):
                contacts = loaded
                applyFilter()
                if let selection, !loaded.contains(where: { $0.contactId == selection }) {
                    self.selection = nil
                }
            case let .failure(error):
                record(error, "contacts not read")
            }
        }
    }

    private func reloadCard() {
        generations.card += 1
        let generation = generations.card

        guard let id = selection else {
            card = nil
            return
        }
        if card?.contactId != id {
            card = nil
        }

        reads.read({ [api] in try api.card(contactId: id) }) { [weak self] result in
            guard let self, generation == self.generations.card else { return }
            switch result {
            case let .success(loaded):
                card = loaded
            case let .failure(error):
                record(error, "contact not read")
            }
        }
    }

    // MARK: - Search

    private func scheduleFilter() {
        filterTask?.cancel()
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)

        // Clearing the field shows the whole list at once.
        guard !query.isEmpty, timing.searchDelay > .zero else {
            appliedQuery = query
            applyFilter()
            return
        }

        let delay = timing.searchDelay
        filterTask = Task { [weak self] in
            try? await Task.sleep(for: delay)
            guard !Task.isCancelled, let self else { return }
            appliedQuery = query
            applyFilter()
        }
    }

    private func applyFilter() {
        let query = appliedQuery
        let shown = query.isEmpty ? contacts : contacts.filter { ContactsIndex.matches($0, query: query) }
        sections = ContactsIndex.sections(shown)
    }

    // MARK: - Failures

    /// A read that failed: the last data stays shown.
    private func record(_ error: any Error, _ subject: StaticString) {
        guard !Self.notStarted(error) else { return }
        log.error("\(subject.description, privacy: .public): \(error: error)")
    }

    private static func notStarted(_ error: any Error) -> Bool {
        if case FeatureApiError.NotStarted? = error as? FeatureApiError { true } else { false }
    }
}

/// The newest read of each kind; an older one's result is dropped.
private struct ContactsGenerations {
    var state = 0
    var list = 0
    var card = 0
}
