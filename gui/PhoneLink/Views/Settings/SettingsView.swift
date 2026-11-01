import AppKit
import PhoneLinkKit
import SwiftUI

/// The Settings window (HIG › Settings): a fixed toolbar of panes, the window titled after the active
/// one, reopening on the pane shown last.
struct SettingsView: View {
    @Environment(SettingsModel.self) private var model
    @AppStorage(PreferenceKeys.settingsPane) private var pane = SettingsPane.general

    var body: some View {
        TabView(selection: $pane) {
            SettingsGeneralPane()
                .tabItem { Label("General", systemImage: "gearshape") }
                .tag(SettingsPane.general)
            SettingsNotificationsPane()
                .tabItem { Label("Notifications", systemImage: "bell.badge") }
                .tag(SettingsPane.notifications)
            SettingsMessagesPane()
                .tabItem { Label("Messages", systemImage: "message") }
                .tag(SettingsPane.messages)
            SettingsPhotosPane()
                .tabItem { Label("Photos", systemImage: "photo.on.rectangle") }
                .tag(SettingsPane.photos)
            SettingsFilesPane()
                .tabItem { Label("Files", systemImage: "doc.on.doc") }
                .tag(SettingsPane.files)
            SettingsCameraPane()
                .tabItem { Label("Camera", systemImage: "web.camera") }
                .tag(SettingsPane.camera)
            SettingsMirroringPane()
                .tabItem { Label("Phone Screen", systemImage: "apps.iphone") }
                .tag(SettingsPane.mirroring)
            SettingsAccountPane()
                .tabItem { Label("Account", systemImage: "person.crop.circle") }
                .tag(SettingsPane.account)
            SettingsAdvancedPane()
                .tabItem { Label("Advanced", systemImage: "gearshape.2") }
                .tag(SettingsPane.advanced)
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
            .frame(width: 640)
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
