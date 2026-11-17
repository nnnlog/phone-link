import PhoneLinkKit
import SwiftUI

/// Settings › General › Updates: the running version, the daily check and its channel, and a check now.
struct SettingsUpdatesSection: View {
    @Environment(UpdateModel.self) private var updates

    var body: some View {
        @Bindable var updates = updates
        Section {
            LabeledContent("Version", value: updates.current.description)
            if updates.isEnabled {
                Toggle("Check for updates automatically", isOn: $updates.checksAutomatically)
                Toggle(isOn: includesBetas) {
                    Text("Include beta versions")
                    Text("Beta versions get new features first and may have more problems.")
                }
                LabeledContent {
                    if let release = updates.offered {
                        Button("Update to \(release.version.description)…") { updates.showWindow() }
                    } else {
                        Button("Check Now") { updates.checkNow() }
                            .disabled(updates.phase == .checking)
                    }
                } label: {
                    Text(status)
                }
            }
        } header: {
            Text("Updates")
        } footer: {
            if !updates.isEnabled {
                Text("This build of Phone Link doesn't check for updates.")
            }
        }
    }

    private var includesBetas: Binding<Bool> {
        Binding(
            get: { updates.channel == .beta },
            set: { updates.channel = $0 ? .beta : .stable }
        )
    }

    private var status: String {
        switch updates.phase {
        case .checking:
            return String(localized: "Checking…")
        case let .available(release), let .downloading(release, _), let .installing(release):
            return String(localized: "Version \(release.version.description) is available.")
        case .failed:
            return String(localized: "The last check didn't finish.")
        case .idle, .upToDate:
            guard let lastChecked = updates.lastChecked else {
                return String(localized: "Not checked yet")
            }
            return String(localized: "Last checked \(lastChecked.formatted(.relative(presentation: .named)))")
        }
    }
}
