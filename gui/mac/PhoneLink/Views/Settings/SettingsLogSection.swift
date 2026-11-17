import AppKit
import PhoneLinkKit
import SwiftUI
import UniformTypeIdentifiers

private let log = AppLog(category: "settings")

/// Settings › Advanced › Log: the size of the app's log, a copy saved where the user chooses (then
/// shown in Finder), and clearing it. The log is there from launch, signed in or not.
struct SettingsLogSection: View {
    @State private var size = LogArchive.size
    @State private var saving = false
    @State private var confirmingClear = false
    @State private var failure: Failure?

    private struct Failure {
        let title: String
        let message: String
    }

    var body: some View {
        Section {
            LabeledContent("Log Size", value: ByteCountFormatter.string(fromByteCount: Int64(size), countStyle: .file))
            HStack {
                Button("Save Log…") { save() }
                    .disabled(saving)
                Button("Clear Log…", role: .destructive) { confirmingClear = true }
                    .disabled(size == 0)
            }
        } header: {
            Text("Log")
        } footer: {
            Text(
                "Phone Link records what it does on this Mac to help diagnose problems. Account and device IDs, names, numbers, and other personal details are replaced with REDACTED tags as they are recorded. The log stays on this Mac until you save a copy."
            )
        }
        .onAppear { size = LogArchive.size }
        .confirmationDialog("Clear the Log?", isPresented: $confirmingClear, titleVisibility: .visible) {
            Button("Clear", role: .destructive) { clear() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Everything Phone Link has recorded so far will be deleted.")
        }
        .alert(failure?.title ?? "", isPresented: failurePresented, presenting: failure) { _ in
        } message: { failure in
            Text(failure.message)
        }
    }

    private var failurePresented: Binding<Bool> {
        Binding(get: { failure != nil }, set: { if !$0 { failure = nil } })
    }

    private func save() {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = LogArchive.suggestedFileName(at: Date())
        panel.allowedContentTypes = [.log]
        panel.canCreateDirectories = true
        panel.isExtensionHidden = false

        saving = true
        Task {
            defer { saving = false }
            let response: NSApplication.ModalResponse
            if let window = NSApp.keyWindow {
                response = await panel.beginSheetModal(for: window)
            } else {
                response = panel.runModal()
            }

            guard response == .OK, let destination = panel.url else { return }
            do {
                try await LogArchive.save(to: destination)
                NSWorkspace.shared.activateFileViewerSelecting([destination])
            } catch {
                log.error("log not saved: \(error: error)")
                failure = Failure(title: String(localized: "Phone Link Couldn't Save the Log"), message: UserText.describe(error))
            }

            size = LogArchive.size
        }
    }

    private func clear() {
        do {
            try LogArchive.clear()
        } catch {
            log.error("log not cleared: \(error: error)")
            failure = Failure(title: String(localized: "Phone Link Couldn't Clear the Log"), message: UserText.describe(error))
        }

        size = LogArchive.size
    }
}
