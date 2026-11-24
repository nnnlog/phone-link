import AppKit
import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI
import UniformTypeIdentifiers
import Vision

private let log = AppLog(category: "messages")

/// What can be done with a message's attachment this Mac has the bytes of: each action first writes
/// the attachment to its file in the temporary folder, then hands that file on. A failure is told in
/// the Messages alert.
@MainActor
enum MessagesAttachmentActions {
    /// The picker on screen; kept until the next one replaces it.
    private static var sharingPicker: NSSharingServicePicker?

    /// The attachment's file, or `nil` after telling the user why there is none.
    static func file(of part: MessagePart, model: MessagesModel, failure title: String) async -> URL? {
        do {
            guard let file = try await model.partFile(part) else {
                log.notice("attachment not read; this Mac has none of its bytes")
                model.alert = MessagesAlert(title: title, message: String(localized: "This Mac doesn't have the attachment."))
                return nil
            }
            return file
        } catch {
            log.error("attachment not read: \(error: error)")
            model.alert = MessagesAlert(title: title, message: UserText.describe(error))
            return nil
        }
    }

    /// Opens the attachment in its app, or in `app`.
    static func open(_ part: MessagePart, with app: URL? = nil, model: MessagesModel) {
        Task {
            guard let file = await file(of: part, model: model, failure: String(localized: "Attachment Not Opened")) else { return }

            if let app {
                do {
                    try await NSWorkspace.shared.open([file], withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration())
                } catch {
                    log.error("attachment not opened: \(error: error)")
                    model.alert = MessagesAlert(title: String(localized: "Attachment Not Opened"), message: UserText.describe(error))
                }
            } else if !NSWorkspace.shared.open(file) {
                log.error("attachment not opened; no app opened it")
            }
        }
    }

    /// Open With › Other…: an app from Applications opens the attachment.
    static func chooseApp(_ part: MessagePart, model: MessagesModel) {
        guard let window = NSApp.keyWindow else { return }

        let panel = NSOpenPanel()
        panel.directoryURL = URL(fileURLWithPath: "/Applications", isDirectory: true)
        panel.allowedContentTypes = [.application]
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.prompt = String(localized: "Open")
        panel.message = String(localized: "Choose an app to open “\(MessagesFormat.attachmentFileName(part))”.")

        Task {
            guard await panel.beginSheetModal(for: window) == .OK, let app = panel.url else { return }
            open(part, with: app, model: model)
        }
    }

    /// Copy: the file, and for an image its data too, so it pastes into a document as well as a folder.
    static func copy(_ part: MessagePart, model: MessagesModel) {
        Task {
            let title = String(localized: "Attachment Not Copied")
            guard let file = await file(of: part, model: model, failure: title) else { return }

            let type = MessagesFormat.contentType(of: part)
            let entry = NSPasteboardItem()
            entry.setString(file.absoluteString, forType: .fileURL)
            if type.conforms(to: .image), let data = try? Data(contentsOf: file) {
                entry.setData(data, forType: NSPasteboard.PasteboardType(type.identifier))
            }

            NSPasteboard.general.clearContents()
            if !NSPasteboard.general.writeObjects([entry]) {
                log.error("attachment not copied; the pasteboard refused it")
                model.alert = MessagesAlert(title: title, message: String(localized: "The clipboard didn't accept the attachment."))
            }
        }
    }

    /// Save As…: the save panel with the attachment's name; its bytes are written where the user says.
    static func saveAs(_ part: MessagePart, model: MessagesModel) {
        guard let window = NSApp.keyWindow else { return }

        let type = MessagesFormat.contentType(of: part)
        let panel = NSSavePanel()
        panel.nameFieldStringValue = MessagesFormat.attachmentFileName(part)
        if !type.isDynamic, type != .data {
            panel.allowedContentTypes = [type]
        }
        panel.canCreateDirectories = true
        panel.isExtensionHidden = false

        Task {
            guard await panel.beginSheetModal(for: window) == .OK, let destination = panel.url else { return }

            let title = String(localized: "Attachment Not Saved")
            guard let file = await file(of: part, model: model, failure: title) else { return }
            do {
                try await write(file, to: destination)
            } catch {
                log.error("attachment not saved: \(error: error)")
                model.alert = MessagesAlert(title: title, message: UserText.describe(error))
            }
        }
    }

    @concurrent
    nonisolated private static func write(_ file: URL, to destination: URL) async throws {
        try Data(contentsOf: file).write(to: destination, options: .atomic)
    }

    /// Share…: the share picker at the pointer.
    static func share(_ part: MessagePart, model: MessagesModel) {
        guard let anchor = PhotosAnchor.pointer(in: NSApp.keyWindow) else { return }

        Task {
            guard let file = await file(of: part, model: model, failure: String(localized: "Attachment Not Shared")) else { return }
            guard let window = anchor.window, window.isVisible, let view = window.contentView else { return }

            let point = view.convert(anchor.location, from: nil)
            let picker = NSSharingServicePicker(items: [file])
            sharingPicker = picker
            picker.show(
                relativeTo: NSRect(x: point.x - 1, y: point.y - 1, width: 2, height: 2),
                of: view,
                preferredEdge: view.isFlipped ? .maxY : .minY
            )
        }
    }

    /// Copy Text from Picture: the text the system reads in the image, its lines one under another.
    static func copyText(of part: MessagePart, model: MessagesModel) {
        Task {
            let title = String(localized: "Text Not Copied")
            guard let file = await file(of: part, model: model, failure: title) else { return }

            do {
                let text = try await recognizedText(in: file)
                guard !text.isEmpty else {
                    model.alert = MessagesAlert(title: title, message: String(localized: "No text was found in the picture."))
                    return
                }

                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(text, forType: .string)
            } catch {
                log.error("picture text not read: \(error: error)")
                model.alert = MessagesAlert(title: title, message: UserText.describe(error))
            }
        }
    }

    @concurrent
    nonisolated private static func recognizedText(in file: URL) async throws -> String {
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        request.usesLanguageCorrection = true
        request.automaticallyDetectsLanguage = true

        try VNImageRequestHandler(url: file).perform([request])
        let lines = request.results?.compactMap { $0.topCandidates(1).first?.string } ?? []
        return lines.joined(separator: "\n")
    }

    /// The attachment as a file promise for a drag: written when the drop target asks for it.
    static func dragProvider(_ part: MessagePart, model: MessagesModel) -> NSItemProvider {
        let provider = NSItemProvider()
        provider.suggestedName = MessagesFormat.attachmentFileName(part)

        provider.registerFileRepresentation(
            forTypeIdentifier: MessagesFormat.contentType(of: part).identifier,
            fileOptions: [],
            visibility: .all
        ) { completion in
            Task { @MainActor in
                do {
                    if let file = try await model.partFile(part) {
                        completion(file, false, nil)
                    } else {
                        completion(nil, false, CocoaError(.fileReadNoSuchFile))
                    }
                } catch {
                    log.error("dragged attachment not written: \(error: error)")
                    completion(nil, false, error)
                }
            }
            return nil
        }

        return provider
    }
}

/// An attachment's context menu, in the order of a photo's: Open and Open With, then Copy, Save As…
/// and Share…, and for a picture Copy Text from Picture.
struct MessagesAttachmentMenu: View {
    let part: MessagePart
    let model: MessagesModel

    var body: some View {
        Button("Open") { MessagesAttachmentActions.open(part, model: model) }
        Menu("Open With") {
            let apps = PhotosOpenWithApps.apps(for: MessagesFormat.contentType(of: part))
            ForEach(apps) { app in
                Button {
                    MessagesAttachmentActions.open(part, with: app.url, model: model)
                } label: {
                    Label {
                        Text(app.isDefault ? String(localized: "\(app.name) (default)") : app.name)
                    } icon: {
                        Image(nsImage: app.icon)
                    }
                }
                if app.isDefault && apps.count > 1 {
                    Divider()
                }
            }
            if !apps.isEmpty {
                Divider()
            }
            Button("Other…") { MessagesAttachmentActions.chooseApp(part, model: model) }
        }
        Divider()
        Button("Copy") { MessagesAttachmentActions.copy(part, model: model) }
        Button("Save As…") { MessagesAttachmentActions.saveAs(part, model: model) }
        Button("Share…") { MessagesAttachmentActions.share(part, model: model) }
        if part.state == .image {
            Divider()
            Button("Copy Text from Picture") { MessagesAttachmentActions.copyText(of: part, model: model) }
        }
    }
}
