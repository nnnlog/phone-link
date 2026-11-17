import AppKit
import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The Notifications section's detail column: the selected notification in full, with its buttons
/// and its reply field.
struct NotificationDetailColumn: View {
    @Environment(NotificationsModel.self) private var model

    var body: some View {
        let notification = model.selectedNotification
        Group {
            if let notification {
                NotificationDetail(notification: notification)
                    .id(notification.key)
            } else if model.content == .list {
                ContentUnavailableView("No Notification Selected", systemImage: "bell")
            } else {
                // The content column already says why there is no list.
                Color.clear
            }
        }
        .modifier(NotificationCommands(notification: notification))
    }
}

/// One notification: its app and time, its text, the phone's buttons and links, and at the bottom the
/// reply field when the notification takes a reply.
private struct NotificationDetail: View {
    let notification: PhoneNotification
    @Environment(NotificationsModel.self) private var model
    @Environment(ScreenMirroringModel.self) private var mirroring
    @State private var replyText = ""
    @FocusState private var replyFocused: Bool

    /// The text's widest line: longer lines read poorly in a wide window.
    private static let textWidth: CGFloat = 560

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    header
                    content
                    if hasButtons {
                        // One row while it fits, else one button under another.
                        ViewThatFits(in: .horizontal) {
                            HStack(spacing: 8) { buttons }
                            VStack(alignment: .leading, spacing: 8) { buttons }
                        }
                        .buttonStyle(.bordered)
                    }
                }
                // The text wraps within this width; it must not size itself to its lines, which would
                // widen the window.
                .frame(maxWidth: Self.textWidth, alignment: .leading)
                .padding(24)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            if let reply = notification.reply {
                replyField(placeholder: reply.name)
            }
        }
        .task(id: model.replyFocusRequest) {
            guard model.replyFocusRequest == notification.key else { return }
            replyFocused = true
            model.replyFocusRequest = nil
        }
    }

    /// The app's icon and name over when the notification was posted.
    private var header: some View {
        HStack(spacing: 8) {
            PhoneAppIcon(packageName: notification.packageName, size: 28)
            VStack(alignment: .leading, spacing: 1) {
                Text(NotificationsModel.appName(of: notification))
                    .font(.subheadline.weight(.semibold))
                Text(Self.postedText(NotificationTimeText.date(ofUnixMs: notification.timeMs)))
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .combine)
    }

    /// The title and subtitle, then the body or the conversation.
    private var content: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let title = notification.title, !title.isEmpty {
                Text(title)
                    .font(.title2.weight(.semibold))
                    .textSelection(.enabled)
            }
            if let subtitle = notification.subtitle, !subtitle.isEmpty {
                Text(subtitle)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
            if notification.conversation.isEmpty {
                if !notification.body.isEmpty {
                    Text(notification.body)
                        .textSelection(.enabled)
                        .padding(.top, 4)
                }
            } else {
                NotificationConversation(lines: notification.conversation)
                    .padding(.top, 4)
            }
        }
    }

    private var hasButtons: Bool {
        !notification.actions.isEmpty || NotificationsModel.opensInMessages(notification) || mirroring.canOpen(notification)
    }

    /// The phone's own buttons, then where the notification opens.
    @ViewBuilder
    private var buttons: some View {
        ForEach(notification.actions, id: \.position) { action in
            Button(action.name) { model.press(action, on: notification) }
        }
        if NotificationsModel.opensInMessages(notification) {
            Button("Open in Messages") { model.openInMessages(notification) }
        }
        if mirroring.canOpen(notification) {
            Button("Open on Phone") { mirroring.open(notification) }
                .help("Open it in a window showing your phone")
        }
    }

    /// The reply field, shaped like the Messages composer's: Return sends, as does its button.
    private func replyField(placeholder: String) -> some View {
        HStack(spacing: 6) {
            TextField(placeholder, text: $replyText)
                .textFieldStyle(.plain)
                .focused($replyFocused)
                .onSubmit(send)
            Button(action: send) {
                Label("Send", systemImage: "arrow.up.circle.fill")
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
            .font(.title3)
            .help("Send")
            .disabled(!NotificationsModel.canSend(replyText))
        }
        .padding(.leading, 14)
        .padding(.trailing, 6)
        .padding(.vertical, 6)
        .modifier(ReplyFieldBackground())
        .padding(14)
    }

    private func send() {
        guard NotificationsModel.canSend(replyText) else {
            NSSound.beep()
            return
        }

        if model.sendReply(replyText, to: notification) {
            replyText = ""
        }
    }

    /// "Today at 1:32 PM", "Yesterday at 9:05 AM", else the date and the time, in the user's format.
    private static func postedText(_ date: Date) -> String {
        let formatter = DateFormatter()
        formatter.dateStyle = .medium
        formatter.timeStyle = .short
        formatter.doesRelativeDateFormatting = true
        return formatter.string(from: date)
    }
}

/// The reply field's capsule: Liquid Glass where the system has it, else an outline.
private struct ReplyFieldBackground: ViewModifier {
    func body(content: Content) -> some View {
        if #available(macOS 26, *) {
            content.glassEffect(.regular, in: Capsule())
        } else {
            content.background(Capsule().strokeBorder(.separator))
        }
    }
}

/// The detail's toolbar: Pin and Dismiss at its trailing end, where Mail keeps a message's commands.
private struct NotificationCommands: ViewModifier {
    let notification: PhoneNotification?
    @Environment(NotificationsModel.self) private var model

    func body(content: Content) -> some View {
        if #available(macOS 26, *) {
            // Liquid Glass toolbars start a column's items at its leading edge, and hand the space of a
            // detail column without items to the list column, whose Clear All would then move to the
            // window's edge; the flexible spacer keeps both where they belong.
            content.toolbar {
                ToolbarSpacer(.flexible)
                if let notification {
                    ToolbarItemGroup { buttons(for: notification) }
                }
            }
        } else if let notification {
            content.toolbar {
                ToolbarItemGroup { buttons(for: notification) }
            }
        } else {
            content
        }
    }

    @ViewBuilder
    private func buttons(for notification: PhoneNotification) -> some View {
        Button {
            model.setPinned(notification, !notification.pinned)
        } label: {
            Label(notification.pinned ? "Unpin" : "Pin", systemImage: notification.pinned ? "pin.slash" : "pin")
        }
        .help(notification.pinned ? "Unpin" : "Pin")
        Button {
            model.dismiss(notification)
        } label: {
            Label("Dismiss", systemImage: "xmark")
        }
        .help("Dismiss")
    }
}

/// A messaging-style notification: each sender in bold beside their messages.
private struct NotificationConversation: View {
    let lines: [NotificationLine]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(Array(runs.enumerated()), id: \.offset) { _, run in
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    if let sender = run.sender {
                        Text(sender)
                            .fontWeight(.semibold)
                    }
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(Array(run.messages.enumerated()), id: \.offset) { _, message in
                            Text(message)
                        }
                    }
                }
            }
        }
        .textSelection(.enabled)
    }

    /// The lines as sender runs. A run may have no sender: the engine drops the first heading when it
    /// became the title, and a conversation with a blank sender has no headings at all.
    private var runs: [(sender: String?, messages: [String])] {
        var runs: [(sender: String?, messages: [String])] = []
        for line in lines {
            switch line {
            case let .sender(name):
                runs.append((name, []))
            case let .message(text):
                if runs.isEmpty {
                    runs.append((nil, []))
                }
                runs[runs.count - 1].messages.append(text)
            }
        }
        return runs
    }
}
