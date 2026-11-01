import PhoneLinkKit
import ServiceManagement
import SwiftUI

private let log = AppLog(category: "settings")

/// General: how Phone Link opens and where it shows, what the selected phone shares with this Mac, and
/// the window's appearance.
struct SettingsGeneralPane: View {
    @Environment(SettingsModel.self) private var model
    @Environment(EngineController.self) private var controller
    @Environment(WallpaperModel.self) private var wallpaper
    @AppStorage(PreferenceKeys.showMenuBarExtra) private var showMenuBarExtra = true
    @AppStorage(PreferenceKeys.showWindowAtLaunch) private var showWindowAtLaunch = true
    /// The login item's state as macOS reports it; read when the pane appears and when the app becomes
    /// active (the user changes it in System Settings).
    @State private var loginItem: SMAppService.Status?
    @State private var loginItemFailure: String?

    var body: some View {
        @Bindable var model = model
        @Bindable var wallpaper = wallpaper
        Form {
            Section {
                Toggle(isOn: opensAtLogin) {
                    Text("Open Phone Link at login")
                    if loginItem == .requiresApproval {
                        Text("Allow Phone Link in System Settings › General › Login Items.")
                    }
                }
                if loginItem == .requiresApproval {
                    LabeledContent {
                        Button("Open Login Items Settings") {
                            SMAppService.openSystemSettingsLoginItems()
                        }
                    } label: {
                        EmptyView()
                    }
                }
                Toggle("Open the Phone Link window at launch", isOn: $showWindowAtLaunch)
                Toggle("Show Phone Link in the menu bar", isOn: $showMenuBarExtra)
            }
            Section {
                Toggle(isOn: $model.sharesClipboard) {
                    Text("Share clipboard")
                    if model.clipboardState?.capable == false {
                        Text("This phone doesn't support clipboard sharing.")
                    } else if model.clipboardState?.applicable == false {
                        Text("Clipboard sharing is turned off on this Mac.")
                    } else {
                        Text("Text and images you copy on one device can be pasted on the other.")
                    }
                }
                .disabled(!model.clipboardToggleApplies)
                SettingsRemoteControlRow()
                SettingsMediaControlsRows()
            } header: {
                Text(controller.phone?.name ?? String(localized: "Phone"))
            } footer: {
                if !model.isAvailable {
                    Text("Settings are available once Phone Link is connected.")
                }
            }
            Section {
                Toggle(isOn: $wallpaper.usesAsBackground) {
                    Text("Use phone wallpaper as sidebar background")
                    if wallpaper.transparencyReduced {
                        Text("Shown when Reduce Transparency is off in System Settings › Accessibility › Display.")
                    } else {
                        Text("The phone's wallpaper shows behind the sidebar.")
                    }
                }
            } header: {
                Text("Appearance")
            }
            SettingsUpdatesSection()
        }
        .settingsPaneForm()
        .onAppear {
            loginItem = SMAppService.mainApp.status
            model.reload()
        }
        // Back from approving the login item in System Settings.
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            loginItem = SMAppService.mainApp.status
        }
        .alert(loginItemFailure ?? "", isPresented: loginItemFailurePresented) {
        } message: {
            Text("You can change this in System Settings › General › Login Items.")
        }
    }

    private var opensAtLogin: Binding<Bool> {
        Binding(
            get: { loginItem == .enabled || loginItem == .requiresApproval },
            set: { open in
                do {
                    if open {
                        try SMAppService.mainApp.register()
                    } else {
                        try SMAppService.mainApp.unregister()
                    }
                } catch {
                    log.error("login item not \(open ? "added" : "removed", privacy: .public): \(error: error)")
                    loginItemFailure = open
                        ? String(localized: "Phone Link Couldn't Be Added to Login Items")
                        : String(localized: "Phone Link Couldn't Be Removed from Login Items")
                }
                loginItem = SMAppService.mainApp.status
            }
        )
    }

    private var loginItemFailurePresented: Binding<Bool> {
        Binding(get: { loginItemFailure != nil }, set: { if !$0 { loginItemFailure = nil } })
    }
}
