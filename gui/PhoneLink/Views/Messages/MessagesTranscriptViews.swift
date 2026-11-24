import AppKit
import PhoneLinkEngine
import PhoneLinkKit
import QuickLook
import SwiftUI

/// Messages oldest first, opened at the newest and following the ones that arrive.
struct MessagesTranscriptList: View {
    let rows: [MessagesTranscriptRow]

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(spacing: 6) {
                    ForEach(rows) { row in
                        MessagesTranscriptItem(row: row)
                    }
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 12)
            }
            .modifier(MessagesBottomAnchor())
            .onChange(of: rows.last?.id) { _, newest in
                guard let newest else { return }
                withAnimation {
                    proxy.scrollTo(newest, anchor: .bottom)
                }
            }
        }
    }
}

/// Opens a transcript at its newest message and, where the system can (macOS 15), keeps it there while
/// the transcript's size changes — the composer growing a line, a new message arriving.
private struct MessagesBottomAnchor: ViewModifier {
    func body(content: Content) -> some View {
        if #available(macOS 15.0, *) {
            content
                .defaultScrollAnchor(.bottom)
                .defaultScrollAnchor(.bottom, for: .sizeChanges)
        } else {
            content.defaultScrollAnchor(.bottom)
        }
    }
}

/// A message with the day and time above it when it starts a new stretch; a group event ("X joined")
/// as centered text.
private struct MessagesTranscriptItem: View {
    let row: MessagesTranscriptRow

    var body: some View {
        VStack(spacing: 6) {
            if let separator = row.separator {
                Text(separator)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity)
                    .padding(.top, 6)
            }
            if row.message.groupEvent != nil {
                Text(MessagesFormat.groupEvent(row.message))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: .infinity)
            } else {
                MessagesBubbleRow(row: row)
            }
        }
    }
}

/// A received message on the left in gray, a sent one on the right in the accent color: its
/// attachments, then its text, then how its send went — for one sent from this Mac its own state (with
/// Try Again when it failed), for one the phone holds unsent its mailbox.
private struct MessagesBubbleRow: View {
    let row: MessagesTranscriptRow
    @Environment(MessagesModel.self) private var model

    private var message: Message { row.message }
    private var outgoing: Bool { !message.incoming }

    var body: some View {
        HStack(alignment: .center, spacing: 6) {
            if outgoing {
                Spacer(minLength: 48)
            }
            VStack(alignment: outgoing ? .trailing : .leading, spacing: 3) {
                if let sender = row.sender {
                    Text(sender)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 12)
                }
                ForEach(message.parts, id: \.part) { part in
                    MessagesPartView(part: part)
                }
                if !message.body.isEmpty || !message.subject.isEmpty {
                    textBubble
                }
                sendStatus
            }
            .frame(maxWidth: 520, alignment: outgoing ? .trailing : .leading)
            if case .failed = message.sendState {
                Button {
                    Task { await model.retry(message.id) }
                } label: {
                    Label("Try Again", systemImage: "exclamationmark.circle.fill")
                }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .font(.title3)
                .foregroundStyle(.red)
                .help("Try Again")
                .disabled(model.retrying.contains(message.id))
            }
            if !outgoing {
                Spacer(minLength: 48)
            }
        }
    }

    /// The subject over the body. The body's web and e-mail addresses are links, and its menu copies
    /// the message's text or a link's address.
    private var textBubble: some View {
        VStack(alignment: .leading, spacing: 2) {
            if !message.subject.isEmpty {
                Text(message.subject)
                    .bold()
                    .textSelection(.enabled)
            }
            if !message.body.isEmpty {
                LinkedText(
                    text: message.body,
                    color: outgoing ? .white : .labelColor,
                    linkColor: outgoing ? .white : .linkColor,
                    selectionColor: outgoing ? NSColor.white.withAlphaComponent(0.3) : nil,
                    copyText: MessagesFormat.copyText(of: message)
                )
            }
        }
        .foregroundStyle(outgoing ? AnyShapeStyle(.white) : AnyShapeStyle(.primary))
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
        .background(
            outgoing ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.quaternary),
            in: RoundedRectangle(cornerRadius: 16, style: .continuous)
        )
    }

    @ViewBuilder
    private var sendStatus: some View {
        switch message.sendState {
        case .sending:
            Text("Sending…")
                .font(.caption)
                .foregroundStyle(.secondary)
        case let .failed(reason):
            Text(MessagesFormat.notDelivered(reason))
                .font(.caption)
                .foregroundStyle(.red)
        case .sent:
            EmptyView()
        case nil:
            // The phone's own state of a message it has not delivered; it retries there, not here.
            if let status = MessagesFormat.phoneStatus(message) {
                Text(status)
                    .font(.caption)
                    .foregroundStyle(message.mailbox == .failed ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
            }
        }
    }
}

/// One attachment of a message: an image inline (decoded off the main actor; its row until then), or a
/// row saying what it is with what can be done. One this Mac has the bytes of can be dragged out as a
/// file and has a menu (`MessagesAttachmentMenu`); a click on an image shows it in Quick Look.
private struct MessagesPartView: View {
    let part: MessagePart
    @Environment(MessagesModel.self) private var model
    @State private var preview: URL?

    var body: some View {
        switch part.state {
        case .image:
            MessagesAsyncImage(picture: .part(part.part, size: part.size), maxPixels: 640) { image in
                if let image {
                    Image(nsImage: image)
                        .resizable()
                        .scaledToFit()
                        .frame(maxWidth: 240, maxHeight: 320)
                        .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
                        .accessibilityLabel("Image")
                        .accessibilityAddTraits(.isButton)
                        .onTapGesture(perform: showPreview)
                } else {
                    MessagesAttachmentRow(part: part) { EmptyView() }
                }
            }
            .quickLookPreview($preview)
            .modifier(MessagesAttachmentFile(part: part, model: model))
        case .downloadable:
            MessagesAttachmentRow(part: part) {
                if model.downloading.contains(part.part) {
                    ProgressView()
                        .controlSize(.small)
                } else {
                    Button("Download") {
                        Task { await model.download(part.part) }
                    }
                    .controlSize(.small)
                }
            }
        case .playable, .openable:
            MessagesAttachmentRow(part: part) {
                Button("Open") { MessagesAttachmentActions.open(part, model: model) }
                    .controlSize(.small)
            }
            .modifier(MessagesAttachmentFile(part: part, model: model))
        case .unsupported:
            Text("Attachment not supported")
                .font(.callout)
                .foregroundStyle(.secondary)
                .padding(.horizontal, 12)
                .padding(.vertical, 7)
                .background(.quaternary, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        }
    }

    private func showPreview() {
        Task {
            preview = await MessagesAttachmentActions.file(of: part, model: model, failure: String(localized: "Attachment Not Opened"))
        }
    }
}

/// What an attachment with its bytes on this Mac offers as a file: its menu and a drag out of the window.
private struct MessagesAttachmentFile: ViewModifier {
    let part: MessagePart
    let model: MessagesModel

    func body(content: Content) -> some View {
        content
            .contextMenu { MessagesAttachmentMenu(part: part, model: model) }
            .onDrag { MessagesAttachmentActions.dragProvider(part, model: model) }
    }
}

/// An attachment shown as a row: its kind and size, and a control.
private struct MessagesAttachmentRow<Control: View>: View {
    let part: MessagePart
    @ViewBuilder let control: Control

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: symbol)
                .foregroundStyle(.secondary)
                .accessibilityHidden(true)
            Text(MessagesFormat.attachmentTitle(part))
            control
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(.quaternary, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
    }

    private var symbol: String {
        switch MessagesAttachmentKind(contentType: part.contentType) {
        case .image: "photo"
        case .video: "film"
        case .audio: "waveform"
        case .contact: "person.crop.rectangle"
        case .text: "doc.text"
        case .other: "doc"
        }
    }
}
