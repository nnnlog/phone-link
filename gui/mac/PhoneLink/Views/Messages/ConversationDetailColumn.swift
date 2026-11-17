import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The Messages section's detail column: the shown conversation, the new message, or what was sent to
/// new recipients, each with its composer. It tells the engine what its mark-as-read triggers need —
/// the section is on screen, the window is key — opens the conversation a notification asked for, and
/// shows the failures of the section's actions.
struct ConversationDetailColumn: View {
    @Environment(MessagesModel.self) private var model
    @Environment(AppNavigator.self) private var navigator
    @Environment(\.appearsActive) private var appearsActive

    var body: some View {
        ZStack {
            content
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear { model.setMessagesViewActive(true) }
        .onDisappear { model.setMessagesViewActive(false) }
        .onChange(of: appearsActive, initial: true) { _, active in
            model.setWindowFocused(active)
        }
        .onChange(of: navigator.conversationRequest, initial: true) { _, request in
            guard let request else { return }
            model.select(request)
            navigator.conversationRequest = nil
        }
        .onChange(of: navigator.messageRequest, initial: true) { _, request in
            guard let request else { return }
            model.write(to: request)
            navigator.messageRequest = nil
        }
        .alert(model.alert?.title ?? "", isPresented: alertShown, presenting: model.alert) { _ in
            Button("OK") {}
        } message: { alert in
            Text(alert.message)
        }
    }

    @ViewBuilder
    private var content: some View {
        if model.isGated {
            // The list column shows what to do.
            Color.clear
        } else if model.detail != nil {
            MessagesDetailView()
        } else {
            Text("No Conversation Selected")
                .font(.title3)
                .foregroundStyle(.secondary)
        }
    }

    private var alertShown: Binding<Bool> {
        Binding(get: { model.alert != nil }, set: { if !$0 { model.alert = nil } })
    }
}

/// One detail — a conversation, the new message, or what was sent to new recipients — with its header,
/// its messages (or the contacts matching "To:") and the composer. The composer keeps its place for
/// every detail, so typing goes on uninterrupted when a new message's conversation takes the new
/// message's place. Images dropped on it join the draft.
private struct MessagesDetailView: View {
    @Environment(MessagesModel.self) private var model
    @FocusState private var recipientFieldFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            main
            MessagesComposerBar(onEscape: model.detail == .newMessage ? { model.cancelNewMessage() } : nil)
        }
        .safeAreaInset(edge: .top, spacing: 0) {
            header
        }
        .dropDestination(for: MessagesDroppedImage.self) { items, _ in
            model.addAttachments(items.map(\.attachment))
        }
        .onChange(of: model.detail == .newMessage, initial: true) { _, isNewMessage in
            if isNewMessage {
                recipientFieldFocused = true
            }
        }
    }

    @ViewBuilder
    private var main: some View {
        if !model.suggestions.isEmpty {
            MessagesSuggestionList {
                recipientFieldFocused = true
            }
        } else if model.transcriptFailed {
            ContentUnavailableView {
                Label("Messages Couldn't Be Read", systemImage: "exclamationmark.bubble")
            } description: {
                Text("Phone Link couldn't read this conversation's messages.")
            } actions: {
                Button("Try Again") { model.reloadTranscript() }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            MessagesTranscriptList(rows: model.transcript)
                .id(model.detail)
        }
    }

    @ViewBuilder
    private var header: some View {
        switch model.detail {
        case .conversation:
            if let conversation = model.selectedConversation {
                MessagesConversationHeader(conversation: conversation)
            }
        case .newMessage, .unsent:
            MessagesRecipientHeader(fieldFocused: $recipientFieldFocused)
        case nil:
            EmptyView()
        }
    }
}

/// Who the conversation is with, pinned above the messages, and *Call* for a one-person conversation
/// while call control is offered.
private struct MessagesConversationHeader: View {
    let conversation: Conversation
    @Environment(CallControlModel.self) private var calls

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                MessagesAvatar(conversation: conversation, size: 28)
                VStack(alignment: .leading, spacing: 0) {
                    Text(MessagesFormat.title(conversation))
                        .font(.headline)
                        .lineLimit(2)
                    if let subtitle = MessagesFormat.subtitle(conversation) {
                        Text(subtitle)
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                    }
                }
                Spacer(minLength: 0)
                if let number = callableNumber {
                    CallButton(number: number, iconOnly: true)
                        .buttonStyle(.borderless)
                }
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 8)
            Divider()
        }
        .background(.bar)
    }

    /// The one recipient's address, when it is a phone number and calls may be placed from this Mac.
    private var callableNumber: String? {
        guard calls.isOffered, calls.hasCallHistory, !conversation.isGroup, let recipient = conversation.recipients.first,
              !recipient.address.contains("@")
        else { return nil }
        return recipient.address
    }
}
