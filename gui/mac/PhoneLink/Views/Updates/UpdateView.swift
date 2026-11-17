import AppKit
import PhoneLinkKit
import SwiftUI

/// The update window: what a check found, the offered release's notes, and installing it.
struct UpdateView: View {
    @Environment(UpdateModel.self) private var model
    let close: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(alignment: .top, spacing: 14) {
                Image(nsImage: NSApp.applicationIconImage)
                    .resizable()
                    .frame(width: 64, height: 64)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 4) {
                    Text(title).font(.headline)
                    Text(detail)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            if let release = model.offered {
                ScrollView {
                    Text(Self.notes(release.notes))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .textSelection(.enabled)
                        .padding(10)
                }
                .frame(minHeight: 160, maxHeight: 280)
                .background(.background, in: .rect(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(.separator))
            }
            if case let .downloading(_, fraction) = model.phase {
                ProgressView(value: fraction)
            } else if case .installing = model.phase {
                ProgressView().progressViewStyle(.linear)
            } else if case .checking = model.phase {
                ProgressView().progressViewStyle(.linear)
            }
            buttons
        }
        .padding(20)
        .frame(width: 520)
    }

    @ViewBuilder private var buttons: some View {
        HStack {
            switch model.phase {
            case let .available(release):
                Button("Skip This Version") {
                    model.skip()
                    close()
                }
                Spacer()
                Button("Remind Me Later", action: close)
                    .keyboardShortcut(.cancelAction)
                if model.installability == .available {
                    Button("Install and Relaunch") { model.install() }
                        .keyboardShortcut(.defaultAction)
                } else {
                    Button("Open Release Page") {
                        NSWorkspace.shared.open(release.page)
                        close()
                    }
                    .keyboardShortcut(.defaultAction)
                }
            case .downloading:
                Spacer()
                Button("Cancel") { model.cancel() }
                    .keyboardShortcut(.cancelAction)
            case .installing:
                Spacer()
            case .failed:
                Spacer()
                Button("Close", action: close)
                    .keyboardShortcut(.cancelAction)
                Button("Try Again") { model.checkNow() }
                    .keyboardShortcut(.defaultAction)
            case .idle, .checking, .upToDate:
                Spacer()
                Button("OK", action: close)
                    .keyboardShortcut(.defaultAction)
            }
        }
    }

    private var title: String {
        switch model.phase {
        case .idle, .checking:
            String(localized: "Checking for Updates…")
        case .upToDate:
            String(localized: "Phone Link Is Up to Date")
        case let .available(release), let .downloading(release, _):
            String(localized: "Phone Link \(release.version.description) Is Available")
        case let .installing(release):
            String(localized: "Installing Phone Link \(release.version.description)…")
        case .failed:
            String(localized: "Phone Link Couldn't Update")
        }
    }

    private var detail: String {
        switch model.phase {
        case .idle, .checking:
            String(localized: "Looking for a newer version on GitHub.")
        case .upToDate:
            String(localized: "Version \(model.current.description) is the newest version.")
        case .available, .downloading:
            switch model.installability {
            case .available:
                String(localized: "You have version \(model.current.description). Phone Link relaunches after installing it, which briefly disconnects your phone.")
            case .unsigned:
                String(localized: "You have version \(model.current.description). This copy of Phone Link wasn't signed by its developer, so it can't check and install the update itself. Download it from its release page.")
            case let .notReplaceable(folder):
                String(localized: "You have version \(model.current.description). Phone Link can't replace itself in \(folder). Move it to Applications, or download the update from its release page.")
            }
        case .installing:
            String(localized: "Phone Link relaunches in a moment.")
        case let .failed(message):
            message
        }
    }

    /// GitHub release notes, with their Markdown's inline styles and links.
    private static func notes(_ markdown: String) -> AttributedString {
        if markdown.isEmpty {
            return AttributedString(String(localized: "This release has no notes."))
        }
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        return (try? AttributedString(markdown: markdown, options: options)) ?? AttributedString(markdown)
    }
}
