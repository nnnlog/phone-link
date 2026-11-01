import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The Notifications section's content column: the phone's notifications — pinned ones first, then
/// the rest by day, newest first — or the state that stands in for them (access on the phone, the
/// Mac's switch, nothing yet).
///
/// Text from the phone (its name, app names) is never a localized key: it is shown as is, or as a value
/// inside the app's own text.
struct NotificationListColumn: View {
    @Environment(NotificationsModel.self) private var model
    @Environment(EngineController.self) private var controller
    @Environment(AppNavigator.self) private var navigator
    /// Clear All waits for confirmation: the phone clears the notifications too, for good.
    @State private var confirmingClearAll = false

    var body: some View {
        @Bindable var model = model
        content
            .navigationSplitViewColumnWidth(min: 280, ideal: 300, max: 380)
            .navigationTitle("Notifications")
            .toolbar {
                if showsList {
                    ToolbarItem(placement: .primaryAction) {
                        NotificationPauseToggle()
                    }
                    ToolbarItem(placement: .primaryAction) {
                        // No ellipsis: the button only asks for confirmation (HIG › Buttons). The
                        // smaller text, padded and at its own width, stays clear of the capsule's ends.
                        Button { confirmingClearAll = true } label: {
                            Text("Clear All")
                                .font(.callout)
                                .fixedSize()
                                .padding(.horizontal, 4)
                        }
                        .disabled(!model.canClearAll)
                    }
                }
            }
            .confirmationDialog("Clear All Notifications?", isPresented: $confirmingClearAll) {
                Button("Clear All", role: .destructive) { model.clearAll() }
            } message: {
                Text("Unpinned notifications are also cleared on \(phoneName). This can't be undone.")
            }
            .sheet(item: $model.postNotificationHelp) { help in
                PostNotificationSheet(help: help, phoneName: phoneName)
            }
            .onChange(of: navigator.notificationRequest, initial: true) { _, key in
                guard let key else { return }
                model.select(key)
                navigator.notificationRequest = nil
            }
            .alert(
                model.lastFailure?.title ?? "",
                isPresented: Binding(get: { model.lastFailure != nil }, set: { if !$0 { model.lastFailure = nil } }),
                presenting: model.lastFailure
            ) { _ in
                Button("OK") {}
            } message: { failure in
                Text(failure.message)
            }
    }

    private var phoneName: String {
        controller.phone?.name ?? String(localized: "Phone")
    }

    /// A list is shown, possibly empty or still syncing (the setup states have no list to clear).
    private var showsList: Bool {
        switch model.content {
        case .list, .empty, .syncing: true
        case .loading, .unavailable, .needsPostNotificationPermission, .turnedOff, .lowMemoryPhone, .needsAccess, .failed:
            false
        }
    }

    @ViewBuilder
    private var content: some View {
        switch model.content {
        case .loading:
            ProgressView()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .unavailable:
            ContentUnavailableView {
                Label("Notifications Aren't Available", systemImage: "bell.slash")
            } description: {
                Text("\(phoneName) doesn't share its notifications with Phone Link.")
            }
        case .needsPostNotificationPermission:
            ContentUnavailableView {
                Label("Allow Link to Windows to Send Notifications", systemImage: "bell.badge")
            } description: {
                Text("Notifications from \(phoneName) appear here after you allow Link to Windows to send notifications on the phone.")
            } actions: {
                Button("Show Me How…") { model.showPostNotificationHelp() }
                    .buttonStyle(.borderedProminent)
            }
        case .turnedOff:
            ContentUnavailableView {
                Label("Notifications Are Off on This Mac", systemImage: "bell.slash")
            } description: {
                Text("Turn them on to see the notifications from \(phoneName) here.")
            } actions: {
                Button("Turn On") { model.turnOnNotifications() }
                    .buttonStyle(.borderedProminent)
            }
        case .lowMemoryPhone:
            ContentUnavailableView {
                Label("Notifications Aren't Supported", systemImage: "bell.slash")
            } description: {
                Text("\(phoneName) doesn't have enough memory to share its notifications.")
            }
        case .needsAccess:
            ContentUnavailableView {
                Label("Allow Notification Access on Your Phone", systemImage: "bell")
            } description: {
                Text("Notifications from \(phoneName) appear here after you allow access on the phone.")
            } actions: {
                Button(model.isRequestingAccess ? "Waiting for Your Phone…" : "Send Request to Phone") {
                    model.requestAccess()
                }
                .buttonStyle(.borderedProminent)
                .disabled(model.isRequestingAccess)
            }
        case .syncing:
            ProgressView("Getting Notifications…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .empty:
            ContentUnavailableView {
                Label("No Notifications", systemImage: "bell")
            } description: {
                Text("New notifications from \(phoneName) appear here.")
            }
        case .list:
            NotificationList()
        case let .failed(message):
            ContentUnavailableView {
                Label("Couldn't Show Notifications", systemImage: "exclamationmark.triangle")
            } description: {
                Text(verbatim: message)
            } actions: {
                Button("Try Again") { model.reload() }
            }
        }
    }
}

/// Pauses every notification banner from the phone for 24 hours, or ends the pause. It is the
/// Settings window's switch, so both show what the engine keeps; the help says when the pause ends.
private struct NotificationPauseToggle: View {
    @Environment(SettingsModel.self) private var settings

    var body: some View {
        @Bindable var settings = settings
        Toggle(isOn: $settings.pausesAllNotifications) {
            if settings.pausesAllNotifications {
                Label("Resume Notifications", systemImage: "bell.slash.fill")
            } else {
                Label("Pause Notifications", systemImage: "bell.slash")
            }
        }
        .help(help)
        // As in Settings: the pause applies only while the phone's notifications show here.
        .disabled(!settings.isAvailable || !settings.showsPhoneNotifications)
    }

    private var help: String {
        guard settings.pausesAllNotifications else { return String(localized: "Pause all notifications for 24 hours") }
        guard let until = settings.pausedUntilText else { return String(localized: "Resume Notifications") }
        return String(localized: "Resume Notifications — \(until)")
    }
}

/// The list itself, in sections, selected by notification key.
private struct NotificationList: View {
    @Environment(NotificationsModel.self) private var model
    @Environment(\.undoManager) private var undoManager

    var body: some View {
        @Bindable var model = model
        List(selection: $model.selection) {
            ForEach(model.sections()) { section in
                Section(section.title) {
                    ForEach(section.notifications, id: \.key) { notification in
                        // Only the Pinned section mixes days, so only its rows name the day.
                        NotificationRow(notification: notification, showsDay: section.id == .pinned)
                            .tag(notification.key)
                    }
                }
            }
        }
        .contextMenu(forSelectionType: String.self) { keys in
            if keys.count == 1, let key = keys.first, let notification = model.notifications.first(where: { $0.key == key }) {
                NotificationContextMenu(notification: notification, undoManager: undoManager)
            }
        }
        .onDeleteCommand {
            if let notification = model.selectedNotification {
                model.dismiss(notification)
            }
        }
    }
}

/// A row: the app's icon and name with the time, the title, and two lines of the text. The time is
/// the time of day where the section names the day.
private struct NotificationRow: View {
    let notification: PhoneNotification
    /// The row is in a section that holds several days (Pinned): the time names the day too.
    let showsDay: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                PhoneAppIcon(packageName: notification.packageName, size: 20)
                Text(NotificationsModel.appName(of: notification))
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Spacer(minLength: 4)
                Text(time)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
            if let title = notification.title, !title.isEmpty {
                Text(title)
                    .font(.headline)
                    .lineLimit(1)
            }
            let preview = NotificationsModel.preview(of: notification)
            if !preview.isEmpty {
                Text(preview)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
    }

    private var time: String {
        let date = NotificationTimeText.date(ofUnixMs: notification.timeMs)
        return showsDay ? NotificationTimeText.short(date) : date.formatted(date: .omitted, time: .shortened)
    }
}

/// A row's commands: the notification's own first, then its app's.
private struct NotificationContextMenu: View {
    let notification: PhoneNotification
    let undoManager: UndoManager?
    @Environment(NotificationsModel.self) private var model
    @Environment(ScreenMirroringModel.self) private var mirroring

    var body: some View {
        let app = NotificationsModel.appName(of: notification)
        if NotificationsModel.opensInMessages(notification) {
            Button("Open in Messages") { model.openInMessages(notification) }
        }
        if mirroring.canOpen(notification) {
            Button("Open on Phone") { mirroring.open(notification) }
        }
        if notification.reply != nil {
            Button("Reply…") { model.beginReply(to: notification) }
        }
        ForEach(notification.actions, id: \.position) { action in
            Button(action.name) { model.press(action, on: notification) }
        }
        Button(notification.pinned ? "Unpin" : "Pin") { model.setPinned(notification, !notification.pinned) }

        if model.appCommandsAvailable {
            Divider()
            if model.isAppOn(notification) {
                Button("Turn Off Notifications from \(app)") {
                    model.turnOffApp(of: notification, undoManager: undoManager)
                }
            } else {
                Button("Turn On Notifications from \(app)") { model.turnOnApp(of: notification) }
            }
            if model.canMuteApp(notification) {
                if model.isAppMuted(notification) {
                    Button("Unmute \(app)") { model.unmuteApp(of: notification) }
                } else {
                    Menu("Mute \(app)") {
                        Button("For 1 Hour") { model.muteApp(of: notification, for: .oneHour) }
                        Button("For 8 Hours") { model.muteApp(of: notification, for: .eightHours) }
                        Button("For 24 Hours") { model.muteApp(of: notification, for: .oneDay) }
                    }
                }
            }
        }

        // Destructive, so last (HIG › Context menus); it also dismisses the notification on the phone.
        Divider()
        Button("Dismiss", role: .destructive) { model.dismiss(notification) }
    }
}

/// The post-notification stage's sheet: Link to Windows on the phone may not send notifications, and
/// only the phone can allow it, so the sheet shows the link that opens the setting there — as a code
/// to scan and in its short form. It sends nothing, and closes by itself once the phone reports the
/// permission.
private struct PostNotificationSheet: View {
    let help: PostNotificationHelp
    let phoneName: String
    private let code: CGImage
    @Environment(\.dismiss) private var dismiss

    init(help: PostNotificationHelp, phoneName: String) {
        self.help = help
        self.phoneName = phoneName
        code = help.qrCode()
    }

    var body: some View {
        VStack(spacing: 16) {
            Text("Allow Notifications on Your Phone")
                .font(.title2.weight(.semibold))
            Text("Scan this code with \(phoneName), or open \(PostNotificationHelp.shortLink) on it, then allow Link to Windows to send notifications.")
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
            Image(code, scale: 2, label: Text("QR code that opens \(PostNotificationHelp.shortLink)"))
                .interpolation(.none)
                .resizable()
                .scaledToFit()
                .frame(width: 180, height: 180)
            Text(verbatim: PostNotificationHelp.shortLink)
                .font(.headline)
                .textSelection(.enabled)
            Button("Maybe Later") { dismiss() }
                .keyboardShortcut(.cancelAction)
        }
        .padding(24)
        .frame(width: 380)
    }
}
