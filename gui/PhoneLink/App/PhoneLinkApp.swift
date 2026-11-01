import PhoneLinkKit
import SwiftUI

/// The app: a menu bar extra (the phone's status and quick controls), a Settings window, and the
/// main and sign-in windows the app delegate opens on demand.
@main
struct PhoneLinkApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var app
    @AppStorage(PreferenceKeys.showMenuBarExtra) private var showMenuBarExtra = true

    var body: some Scene {
        MenuBarExtra(isInserted: $showMenuBarExtra) {
            app.environments(PhoneMenu())
        } label: {
            Label("Phone Link", image: "MenuBarIcon")
        }
        .menuBarExtraStyle(.menu)

        Settings {
            app.environments(SettingsView())
        }
        .commands {
            PhoneCommands(app: app)
        }
    }
}

/// The app's own menu in the main menu bar: what the menu bar extra offers, so nothing depends on
/// the extra being shown (HIG › The menu bar › Menu bar extras).
struct PhoneCommands: Commands {
    let app: AppDelegate

    var body: some Commands {
        // Replaces the standard About item, whose panel has no credits, with the menu bar extra's.
        CommandGroup(replacing: .appInfo) {
            Button("About Phone Link") { AboutPanel.show() }
        }
        CommandGroup(after: .appInfo) {
            Button("Check for Updates…") { app.updates.checkNow() }
                .disabled(!app.updates.isEnabled)
        }
        CommandGroup(after: .windowArrangement) {
            Button("Phone Link Window") { app.showMainWindow() }
                .keyboardShortcut("0", modifiers: .command)
        }
        CommandMenu("Phone") {
            // As in the menu bar extra: the controls only while the engine runs with a linked phone.
            if app.controller.phase == .running && !app.controller.phoneUnlinked {
                let controls = PhoneControlGroups(
                    deviceStatus: app.deviceStatus,
                    hotspot: app.hotspot,
                    files: app.fileTransfer,
                    navigator: app.navigator,
                    mirroring: app.mirroring,
                    media: app.mediaControls,
                    separatesFirstGroup: false
                )
                controls
                if controls.showsAnything {
                    Divider()
                }
            }
            Button("Reconnect") { app.controller.refresh() }
                .keyboardShortcut("r", modifiers: [.command, .shift])
        }
    }
}
