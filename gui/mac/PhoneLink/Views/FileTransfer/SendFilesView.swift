import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI
import UniformTypeIdentifiers

/// The send window: drop files on it or choose them, and they go to the phone; the window shows the
/// send's progress, a cancel button and the outcome, and a receive under way. The app adds no entry to
/// the Mac's share menu.
struct SendFilesView: View {
    @Environment(FileTransferModel.self) private var model
    @Environment(EngineController.self) private var controller
    @State private var choosing = false
    @State private var targeted = false

    var body: some View {
        VStack(spacing: 12) {
            if let reason = model.sendUnavailableReason {
                ContentUnavailableView("Can't Send Files", systemImage: "iphone.slash", description: Text(reason))
            } else if model.isSending {
                SendProgressView(model: model)
            } else {
                dropZone
            }
            if let outcome = model.outcome, !model.isSending {
                SendOutcomeView(model: model, outcome: outcome)
            }
            if let receiving = model.receiveStatus {
                Label(receiving, systemImage: "arrow.down.circle")
                    .foregroundStyle(.secondary)
            }
        }
        .padding(16)
        .frame(width: 420)
        // The window takes the content's height, with nothing left over below it.
        .fixedSize(horizontal: false, vertical: true)
        .onAppear { model.reload() }
        .fileImporter(isPresented: $choosing, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
            if case let .success(urls) = result {
                model.sendFromWindow(urls)
            }
        }
        .alert(model.failure?.title ?? "", isPresented: failurePresented, presenting: model.failure) { _ in
        } message: { failure in
            Text(failure.message)
        }
    }

    private var phoneName: String { controller.phone?.name ?? String(localized: "your phone") }

    /// A lightly filled area with a dashed edge, which turns the accent colour while files are
    /// dragged over it.
    private var dropZone: some View {
        VStack(spacing: 10) {
            Image(systemName: "arrow.up.doc")
                .font(.system(size: 30, weight: .light))
                .foregroundStyle(.secondary)
            Text("Drop files to send them to \(phoneName)")
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            Button("Choose Files…") { choosing = true }
                .keyboardShortcut(.defaultAction)
        }
        .padding(16)
        .frame(maxWidth: .infinity, minHeight: 170)
        .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(
                    targeted ? Color.accentColor : Color.primary.opacity(0.12),
                    style: StrokeStyle(lineWidth: 1, dash: [5, 4])
                )
        }
        .dropDestination(for: URL.self) { urls, _ in
            let files = urls.filter(\.isFileURL)
            guard !files.isEmpty else { return false }
            model.sendFromWindow(files)
            return true
        } isTargeted: { targeted = $0 }
    }

    private var failurePresented: Binding<Bool> {
        Binding(get: { model.failure != nil }, set: { if !$0 { model.failure = nil } })
    }
}

/// The running send: the files, what it is doing, its progress, and Cancel.
private struct SendProgressView: View {
    let model: FileTransferModel

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let names = model.transfers.send?.fileNames, !names.isEmpty {
                Text(names.count == 1 ? names[0] : String(localized: "\(names.count) files"))
                    .font(.headline)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            if let fraction = model.sendFraction {
                ProgressView(value: fraction)
            } else {
                ProgressView()
                    .progressViewStyle(.linear)
            }
            HStack {
                Text(model.sendStatus ?? "")
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
                Spacer()
                Button("Cancel") { model.cancelSend() }
                    .keyboardShortcut(.cancelAction)
            }
        }
        .frame(maxWidth: .infinity, minHeight: 170)
    }
}

/// How the last send ended; one that needs the phone's permission offers to ask for it.
private struct SendOutcomeView: View {
    let model: FileTransferModel
    let outcome: FileTransferModel.Outcome

    var body: some View {
        VStack(spacing: 6) {
            Label(outcome.title, systemImage: outcome.succeeded ? "checkmark.circle.fill" : "exclamationmark.circle")
                .font(.headline)
                .foregroundStyle(outcome.succeeded ? Color.green : Color.primary)
            Text(outcome.message)
                .multilineTextAlignment(.center)
                .foregroundStyle(.secondary)
            if outcome.needsPermission {
                Button("Send Request to Phone") {
                    Task { await model.requestPermission() }
                }
            }
        }
    }
}
