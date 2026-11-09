import AppKit
import PhoneLinkKit
import SwiftUI

/// The Settings window (HIG › Settings): a fixed toolbar of panes, the window titled after the active
/// one, reopening on the pane shown last. The pane bar scrolls horizontally, so every pane stays
/// reachable (a `TabView` collapses the last panes into an unusable dropdown).
struct SettingsView: View {
    @Environment(SettingsModel.self) private var model
    @AppStorage(PreferenceKeys.settingsPane) private var pane = SettingsPane.general

    var body: some View {
        VStack(spacing: 0) {
            // Horizontal scrolling tab bar.
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(SettingsPane.allCases, id: \.self) { item in
                        Button {
                            pane = item
                        } label: {
                            HStack(spacing: 5) {
                                Image(systemName: item.symbol)
                                    .font(.system(size: 11, weight: .medium))
                                Text(item.title)
                                    .font(.system(size: 12, weight: .medium))
                            }
                            .foregroundStyle(pane == item ? Color.accentColor : Color.primary)
                            .padding(.horizontal, 12)
                            .frame(height: 30)
                            .background {
                                if pane == item {
                                    Capsule(style: .continuous)
                                        .fill(Color.accentColor.opacity(0.15))
                                }
                            }
                            .overlay {
                                if pane == item {
                                    Capsule(style: .continuous)
                                        .strokeBorder(Color.accentColor.opacity(0.5), lineWidth: 1)
                                }
                            }
                            .contentShape(Capsule())
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
            }

            Divider()

            // The selected pane.
            Group {
                switch pane {
                case .general: SettingsGeneralPane()
                case .notifications: SettingsNotificationsPane()
                case .messages: SettingsMessagesPane()
                case .calls: SettingsCallsPane()
                case .photos: SettingsPhotosPane()
                case .files: SettingsFilesPane()
                case .camera: SettingsCameraPane()
                case .mirroring: SettingsMirroringPane()
                case .account: SettingsAccountPane()
                case .advanced: SettingsAdvancedPane()
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        // Without a window open the app has no Dock icon and is not active: the window would open
        // behind the active app's.
        .onAppear { NSApp.activate() }
        .alert(model.failure?.title ?? "", isPresented: failurePresented, presenting: model.failure) { _ in
        } message: { failure in
            Text(failure.message)
        }
    }

    private var failurePresented: Binding<Bool> {
        Binding(get: { model.failure != nil }, set: { if !$0 { model.failure = nil } })
    }
}

extension View {
    /// A pane's form: grouped, the window's width, and as tall as its rows up to a height past which
    /// it scrolls (the Settings window sizes itself to the pane).
    func settingsPaneForm() -> some View {
        formStyle(.grouped)
            .frame(width: 580)
            .frame(maxHeight: 640)
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// The line a pane shows until the engine runs; its controls are disabled meanwhile.
struct SettingsUnavailableNote: View {
    var body: some View {
        Section {
            Text("Settings are available once Phone Link is connected.")
                .foregroundStyle(.secondary)
        }
    }
}

/// Access the phone gives this Mac: "Allowed", or a button that asks for it on the phone.
struct SettingsAccessRow: View {
    let title: LocalizedStringKey
    let allowed: Bool
    let pending: Bool
    let request: () -> Void

    var body: some View {
        LabeledContent(title) {
            if allowed {
                Text("Allowed")
            } else if pending {
                ProgressView()
                    .controlSize(.small)
                    .accessibilityLabel("Waiting for your phone")
            } else {
                Button("Send Request to Phone", action: request)
            }
        }
    }
}
