pragma Singleton
import Qt.labs.settings

// The shell's own preferences, kept by the app rather than the engine.
Settings {
    category: "Shell"

    // Whether the window opens when Phone Link starts (not at login, which starts it in the tray).
    property bool showWindowAtLaunch: true
    // Whether Phone Link shows its icon in the system tray.
    property bool showTrayIcon: true
    // The one-time notice before the first sign-in: this computer is registered as a Windows device.
    property bool didSeeDeviceRegistrationNotice: false
    // The Settings window's pane shown last, which it reopens on.
    property string settingsPane: "general"
}
