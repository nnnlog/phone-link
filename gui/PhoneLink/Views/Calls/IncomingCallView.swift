import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The incoming-call popup's content: one card per ringing call.
struct IncomingCallView: View {
    @Environment(CallControlModel.self) private var model

    var body: some View {
        VStack(spacing: 8) {
            ForEach(model.popupCalls, id: \.id) { call in
                IncomingCallCard(call: call)
            }
        }
        .padding(8)
        .frame(width: 360)
        .fixedSize(horizontal: false, vertical: true)
    }
}

/// One ringing call: who, on which phone it rings, that the call stays on the phone, and *Decline*,
/// *Message* and *Answer*.
private struct IncomingCallCard: View {
    let call: ActiveCall
    @Environment(CallControlModel.self) private var model
    @Environment(EngineController.self) private var controller

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .top, spacing: 12) {
                ActiveCallAvatar(call: call, size: 44)
                VStack(alignment: .leading, spacing: 2) {
                    Text(call.shownName)
                        .font(.headline)
                        .lineLimit(1)
                    Text(subtitle)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 4)
                Button {
                    model.hidePopup(call)
                } label: {
                    Image(systemName: "xmark")
                }
                .buttonStyle(.borderless)
                .foregroundStyle(.secondary)
                .help("Hide. Your phone keeps ringing.")
            }

            Label("If you answer on this Mac, you'll talk on your phone.", systemImage: "iphone.gen3")
                .font(.callout.weight(.medium))
                .foregroundStyle(.secondary)

            // Solid colors, so the buttons keep them while the panel is not the key window.
            HStack(spacing: 8) {
                Button {
                    Task { await model.decline(call) }
                } label: {
                    Label("Decline", systemImage: "phone.down.fill")
                }
                .buttonStyle(CallActionButtonStyle(fill: .red))

                if model.canReply(to: call) {
                    Menu {
                        ForEach(CallControlModel.cannedReplies, id: \.self) { reply in
                            Button(reply) {
                                Task { await model.reply(to: call, with: reply) }
                            }
                        }
                        Divider()
                        Button("Custom Message…") {
                            Task { await model.replyInMessages(to: call) }
                        }
                    } label: {
                        Image(systemName: "message.fill")
                    }
                    .menuStyle(.borderlessButton)
                    .menuIndicator(.hidden)
                    .frame(width: 44, height: 32)
                    .background(.quaternary, in: Capsule())
                    .accessibilityLabel("Message")
                    .help("Decline and send a message")
                }

                Button {
                    Task { await model.answer(call) }
                } label: {
                    Label("Answer", systemImage: "phone.fill")
                }
                .buttonStyle(CallActionButtonStyle(fill: .green))
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(14)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Incoming call from \(call.shownName)")
    }

    /// A waiting call, else the phone the call rings on.
    private var subtitle: String {
        if call.waiting {
            return String(localized: "Call Waiting")
        }
        guard let phone = controller.phone?.name else {
            return String(localized: "Incoming Call")
        }
        return String(localized: "Incoming call on \(phone)")
    }
}
