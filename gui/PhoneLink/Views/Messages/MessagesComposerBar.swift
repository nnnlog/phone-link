import AppKit
import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI
import UniformTypeIdentifiers

private let log = AppLog(category: "messages")

/// The composer under a conversation or a new message: why sending needs Bluetooth when it does, the
/// draft's images, "+" for more, the text field (Return sends), and the SIM when the phone has two.
struct MessagesComposerBar: View {
    /// Escape in the text field; `true` when it was handled (an empty new message closed).
    var onEscape: (() -> Bool)?
    @Environment(MessagesModel.self) private var model
    @State private var choosingImages = false

    var body: some View {
        let composer = model.composer
        let draft = model.draft
        VStack(alignment: .leading, spacing: 8) {
            if let notice = model.bluetoothNotice {
                Label(notice, systemImage: "exclamationmark.triangle")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
            if !draft.attachments.isEmpty {
                attachments(draft)
            }
            HStack(alignment: .bottom, spacing: 8) {
                Button {
                    choosingImages = true
                } label: {
                    Label("Add Images", systemImage: "plus")
                }
                .labelStyle(.iconOnly)
                .buttonStyle(.bordered)
                .buttonBorderShape(.circle)
                .help("Add Images")
                .disabled(!model.canAddAttachments)
                MessagesComposerField(
                    text: draft.text,
                    placeholder: composer.placeholder,
                    isEditable: composer.isEnabled,
                    onChange: { model.setDraftText($0) },
                    onSubmit: submit,
                    onEscape: onEscape,
                    onPasteImages: composer.isEnabled && composer.canAttach ? { model.addAttachments($0) } : nil
                )
                .padding(.horizontal, 4)
                .background(RoundedRectangle(cornerRadius: 15, style: .continuous).strokeBorder(.separator))
                Button(action: submit) {
                    Image(systemName: "arrow.up.circle.fill")
                        .font(.system(size: 22))
                        .foregroundStyle(model.canSubmit ? Color.accentColor : Color.secondary.opacity(0.4))
                }
                .buttonStyle(.plain)
                .disabled(!model.canSubmit)
                .help("Send Message")
                if model.showsSimPicker {
                    simMenu
                }
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .fileImporter(isPresented: $choosingImages, allowedContentTypes: [.image], allowsMultipleSelection: true) { result in
            switch result {
            case let .success(urls):
                Task {
                    do {
                        model.addAttachments(try await Self.load(urls))
                    } catch {
                        log.error("chosen images not added: \(error: error)")
                        model.alert = MessagesAlert(title: String(localized: "Images Not Added"), message: error.localizedDescription)
                    }
                }
            case let .failure(error):
                log.error("image chooser failed: \(error: error)")
                model.alert = MessagesAlert(title: String(localized: "Images Not Added"), message: error.localizedDescription)
            }
        }
    }

    private func submit() {
        guard model.canSubmit else {
            NSSound.beep()
            return
        }
        Task { await model.send() }
    }

    private func attachments(_ draft: MessagesDraft) -> some View {
        HStack(spacing: 10) {
            ForEach(draft.attachments) { attachment in
                MessagesDraftThumbnail(attachment: attachment)
                    .frame(width: 56, height: 56)
                    .background(.quaternary)
                    .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .accessibilityLabel(attachment.name.isEmpty ? String(localized: "Image") : attachment.name)
                    .overlay(alignment: .topTrailing) {
                        Button {
                            model.removeAttachment(attachment.id)
                        } label: {
                            Label("Remove Image", systemImage: "xmark.circle.fill")
                        }
                        .labelStyle(.iconOnly)
                        .buttonStyle(.borderless)
                        .symbolRenderingMode(.palette)
                        .foregroundStyle(.white, .secondary)
                        .help("Remove Image")
                        .offset(x: 6, y: -6)
                    }
            }
        }
        .padding(.top, 6)
    }

    private var simMenu: some View {
        Menu {
            ForEach(model.subscriptions, id: \.id) { sim in
                Toggle(MessagesFormat.simName(sim), isOn: Binding(
                    get: { model.activeSubscription?.id == sim.id },
                    set: { if $0 { model.selectSubscription(sim.id) } }
                ))
            }
        } label: {
            Label(model.activeSubscription.map(MessagesFormat.simName) ?? "SIM", systemImage: "simcard")
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .help("SIM to Send From")
    }

    /// Reads picked image files off the main actor.
    @concurrent
    nonisolated private static func load(_ urls: [URL]) async throws -> [MessagesAttachment] {
        try urls.map(MessagesAttachment.init(contentsOf:))
    }
}

/// A draft image, decoded off the main actor.
private struct MessagesDraftThumbnail: View {
    let attachment: MessagesAttachment
    @Environment(MessagesModel.self) private var model
    @State private var image: NSImage?

    var body: some View {
        Group {
            if let image {
                Image(nsImage: image)
                    .resizable()
                    .scaledToFill()
            } else {
                Image(systemName: "photo")
                    .foregroundStyle(.secondary)
            }
        }
        .task(id: attachment.id) {
            image = await model.image(of: attachment, maxPixels: 160)
        }
    }
}
