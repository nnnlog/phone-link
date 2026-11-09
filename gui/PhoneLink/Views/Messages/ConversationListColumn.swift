import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The Messages section's content column: the new message being written, what was sent to new
/// recipients that no conversation holds yet, the conversations newest first, and the hidden ones in a
/// section of their own — or the state that stands in for them: messages off on this Mac, not allowed
/// on the phone, the first sync, or none at all.
struct ConversationListColumn: View {
    @Environment(MessagesModel.self) private var model
    @Environment(EngineController.self) private var controller
    @Environment(\.undoManager) private var undoManager
    @State private var showsHidden = false
    @State private var confirmsDiscard = false

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            // Inline Search & New Message Bar (replaces window NSToolbar)
            HStack(spacing: 8) {
                HStack(spacing: 6) {
                    Image(systemName: "magnifyingglass")
                        .foregroundStyle(.secondary)
                        .font(.system(size: 13))
                    TextField("Search", text: $model.searchText)
                        .textFieldStyle(.plain)
                        .font(.system(size: 13))
                    if !model.searchText.isEmpty {
                        Button {
                            model.searchText = ""
                        } label: {
                            Image(systemName: "xmark.circle.fill")
                                .foregroundStyle(.secondary)
                                .font(.system(size: 12))
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 6)
                .background(Color.primary.opacity(0.06), in: RoundedRectangle(cornerRadius: 8, style: .continuous))

                Button {
                    model.startNewMessage()
                } label: {
                    Image(systemName: "square.and.pencil")
                        .font(.system(size: 14, weight: .medium))
                        .foregroundStyle(Color.primary)
                        .frame(width: 28, height: 28)
                        .background(Color.primary.opacity(0.08), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                }
                .buttonStyle(.plain)
                .help("New Message")
                .keyboardShortcut("n")
                .disabled(model.isGated)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)

            Divider()

            content
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .confirmationDialog("Delete This Message?", isPresented: $confirmsDiscard) {
            Button("Delete", role: .destructive) { model.discardNewMessage() }
        } message: {
            Text("The message you're writing and its recipients will be deleted.")
        }
        .navigationSplitViewColumnWidth(min: 260, ideal: 300, max: 420)
    }

    @ViewBuilder
    private var content: some View {
        switch model.listStatus {
        case .turnedOff:
            ContentUnavailableView {
                Label("Messages Are Off on This Mac", systemImage: "message")
            } description: {
                Text("Turn on messages to read and send your phone's texts on this Mac.")
            } actions: {
                Button("Turn On") { model.turnOnMessages() }
                    .buttonStyle(.borderedProminent)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .permissionNeeded:
            ContentUnavailableView {
                Label("Allow Messages on Your Phone", systemImage: "message")
            } description: {
                Text("Messages from \(controller.phone?.name ?? String(localized: "your phone")) appear here after you allow access on the phone.")
            } actions: {
                Button("Send Request to Phone") {
                    Task { await model.requestPermission() }
                }
                .buttonStyle(.borderedProminent)
                .disabled(model.isRequestingPermission)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .syncing:
            ProgressView("Syncing messages…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .empty:
            ContentUnavailableView("No Messages", systemImage: "message", description: Text("Text messages from your phone appear here."))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .conversations:
            conversationList
        }
    }

    private var conversationList: some View {
        @Bindable var model = model
        let unsent = model.listedUnsent
        let conversations = model.visibleConversations
        let hidden = model.hiddenConversations
        let searching = !model.searchText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty

        return List(selection: $model.listSelection) {
            if model.showsNewMessageRow {
                MessagesNewMessageRow(preview: model.newMessagePreview)
                    .tag(MessagesListItem.newMessage)
                    .contextMenu {
                        Button(role: .destructive) {
                            if model.newMessageHasContent {
                                confirmsDiscard = true
                            } else {
                                model.discardNewMessage()
                            }
                        } label: {
                            Label("Delete", systemImage: "trash")
                        }
                    }
            }
            ForEach(unsent) { group in
                MessagesUnsentRow(
                    unsent: group,
                    time: group.latest.map { model.time.listTime(ms: $0.timestampMs) } ?? ""
                )
                .tag(MessagesListItem.unsent(group.id))
            }
            ForEach(conversations, id: \.threadId) { conversation in
                MessagesConversationRow(conversation: conversation, time: model.time.listTime(ms: conversation.latestMs))
                    .tag(MessagesListItem.conversation(conversation.threadId))
                    .contextMenu { contextMenu(for: conversation) }
            }
            if !hidden.isEmpty {
                // Expanded while the search lists hidden conversations too.
                Section("Hidden", isExpanded: Binding(get: { showsHidden || searching }, set: { showsHidden = $0 })) {
                    ForEach(hidden, id: \.threadId) { conversation in
                        MessagesConversationRow(conversation: conversation, time: model.time.listTime(ms: conversation.latestMs))
                            .tag(MessagesListItem.conversation(conversation.threadId))
                            .contextMenu { contextMenu(for: conversation) }
                    }
                }
            }
        }
        .overlay {
            if searching && unsent.isEmpty && conversations.isEmpty && hidden.isEmpty && !model.showsNewMessageRow {
                ContentUnavailableView.search(text: model.searchText)
            }
        }
    }

    @ViewBuilder
    private func contextMenu(for conversation: Conversation) -> some View {
        if conversation.muted {
            Button {
                model.setMuted(false, threadId: conversation.threadId)
            } label: {
                Label("Show Alerts", systemImage: "bell")
            }
        } else {
            Button {
                model.setMuted(true, threadId: conversation.threadId)
            } label: {
                Label("Hide Alerts", systemImage: "bell.slash")
            }
        }
        Divider()
        if conversation.hidden {
            Button {
                model.setHidden(false, threadId: conversation.threadId, undoManager: undoManager)
            } label: {
                Label("Show Conversation", systemImage: "eye")
            }
        } else {
            Button {
                model.setHidden(true, threadId: conversation.threadId, undoManager: undoManager)
            } label: {
                Label("Hide Conversation", systemImage: "eye.slash")
            }
        }
    }
}

/// One conversation of the list: who, the latest message and when, unread and muted marks.
private struct MessagesConversationRow: View {
    let conversation: Conversation
    let time: String
    /// A selected row in a key window is drawn on the accent color: the unread dot turns white there.
    @Environment(\.backgroundProminence) private var prominence

    var body: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(prominence == .increased ? AnyShapeStyle(.primary) : AnyShapeStyle(.tint))
                .frame(width: 8, height: 8)
                .opacity(conversation.unread > 0 ? 1 : 0)
            MessagesAvatar(conversation: conversation, size: 36)
            MessagesRowText(
                title: MessagesFormat.title(conversation),
                time: time,
                preview: MessagesFormat.preview(conversation),
                muted: conversation.muted
            )
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
        .accessibilityValue(conversation.unread > 0 ? Text("Unread") : Text(verbatim: ""))
    }
}

/// What was sent to new recipients: to whom, and how the newest one's send went.
private struct MessagesUnsentRow: View {
    let unsent: MessagesUnsentConversation
    let time: String

    var body: some View {
        HStack(spacing: 8) {
            // The unread dot's place, so the avatars line up.
            Color.clear.frame(width: 8, height: 8)
            MessagesAvatar(recipients: unsent.recipients, size: 36)
            MessagesRowText(title: MessagesFormat.title(unsent), time: time, preview: MessagesFormat.preview(unsent), muted: false)
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
    }
}

/// The new message being written.
private struct MessagesNewMessageRow: View {
    let preview: String

    var body: some View {
        HStack(spacing: 8) {
            Color.clear.frame(width: 8, height: 8)
            Image(systemName: "square.and.pencil")
                .font(.system(size: 15, weight: .medium))
                .foregroundStyle(.secondary)
                .frame(width: 36, height: 36)
                .accessibilityHidden(true)
            MessagesRowText(title: String(localized: "New Message"), time: "", preview: preview, muted: false)
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
    }
}

/// A row's title with its time and muted mark, and the line under it.
private struct MessagesRowText: View {
    let title: String
    let time: String
    let preview: String
    let muted: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Text(title)
                    .font(.headline)
                    .lineLimit(1)
                Spacer(minLength: 4)
                if muted {
                    Image(systemName: "bell.slash.fill")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .accessibilityLabel("Alerts Hidden")
                }
                Text(time)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize()
            }
            Text(preview)
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .lineLimit(2)
        }
    }
}
