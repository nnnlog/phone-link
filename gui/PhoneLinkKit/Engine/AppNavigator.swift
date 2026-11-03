import Foundation
import Observation

/// The main window's sections (its sidebar).
public enum AppSection: String, CaseIterable, Identifiable, Sendable {
    case notifications
    case messages
    /// The phone's recent calls; listed only for an Android phone that has calling.
    case calls
    /// The phone's contacts; listed only for a phone that syncs its contacts.
    case contacts
    case photos
    /// The phone's apps; listed only while the phone opens its apps on this Mac.
    case apps

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .messages: String(localized: "Messages")
        case .calls: String(localized: "Calls")
        case .contacts: String(localized: "Contacts")
        case .photos: String(localized: "Photos")
        case .notifications: String(localized: "Notifications")
        case .apps: String(localized: "Apps")
        }
    }

    /// SF Symbol of the sidebar row.
    public var symbol: String {
        switch self {
        case .messages: "message"
        case .calls: "phone"
        case .contacts: "person.crop.circle"
        case .photos: "photo.on.rectangle"
        case .notifications: "bell"
        case .apps: "square.grid.2x2"
        }
    }
}

/// The Settings window's panes, in toolbar order; the selected one is kept under
/// `PreferenceKeys.settingsPane`.
public enum SettingsPane: String, CaseIterable, Sendable {
    case general
    case notifications
    case messages
    /// Calls: call history and call control.
    case calls
    case photos
    case files
    case camera
    /// Phone Screen: the phone's screen and apps on this Mac.
    case mirroring
    case account
    case advanced
}

/// Where the main window is and where something asked it to go: a notification click, a phone's
/// request to bring the window forward. Models set a request; the main window shows it and the app
/// brings the window to the front.
@MainActor
@Observable
public final class AppNavigator {
    public var section: AppSection = .messages
    /// A conversation to show (a message notification was clicked).
    public var conversationRequest: Int64?
    /// A person to write to (a contact's Message button): their conversation, else a new message.
    public var messageRequest: MessagesRecipient?
    /// A phone notification to show.
    public var notificationRequest: String?
    /// A contact to show (a call's Contact button).
    public var contactRequest: Int64?
    /// Opens the main window and brings it to the front (set by the app).
    @ObservationIgnored public var presentWindow: () -> Void = {}
    /// Opens the sign-in window (set by the app).
    @ObservationIgnored public var presentSignIn: () -> Void = {}
    /// Opens the send-files window (set by the app).
    @ObservationIgnored public var presentSendFiles: () -> Void = {}
    /// Opens the window that pairs a new phone (set by the app).
    @ObservationIgnored public var presentPairing: () -> Void = {}
    /// Opens System Settings › Bluetooth (set by the app).
    @ObservationIgnored public var presentBluetoothSettings: () -> Void = {}

    public init() {}

    public func showWindow() {
        presentWindow()
    }

    public func showSignIn() {
        presentSignIn()
    }

    public func showSendFiles() {
        presentSendFiles()
    }

    public func showPairing() {
        presentPairing()
    }

    public func openBluetoothSettings() {
        presentBluetoothSettings()
    }

    public func show(_ section: AppSection) {
        self.section = section
        showWindow()
    }

    public func showConversation(_ threadId: Int64) {
        conversationRequest = threadId
        show(.messages)
    }

    public func showMessage(to recipient: MessagesRecipient) {
        messageRequest = recipient
        show(.messages)
    }

    public func showContact(_ id: Int64) {
        contactRequest = id
        show(.contacts)
    }

    public func showNotification(_ key: String) {
        notificationRequest = key
        show(.notifications)
    }
}
