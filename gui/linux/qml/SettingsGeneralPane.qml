import QtQuick
import QtQuick.Layouts
import PhoneLink

// General: how Phone Link opens and where it shows, what the selected phone shares with this computer,
// and the window's appearance.
SettingsPane {
    id: pane

    readonly property var settings: { EngineBridge.settingsRevision; return JSON.parse(EngineBridge.query("settings", "")) || {} }
    readonly property bool autostart: { EngineBridge.settingsRevision; return JSON.parse(EngineBridge.query("autostart", "")) === true }
    // The phone's settings can be read: the engine runs.
    readonly property bool phoneSettings: settings.available === true
    // The desktop has a tray (the Settings window knows): the icon's switch applies only then.
    readonly property bool trayAvailable: Window.window !== null && Window.window.trayAvailable === true

    SettingsSection {
        title: L10n.t("General")

        MacSwitch {
            Layout.fillWidth: true
            text: L10n.t("Open Phone Link at login")
            checked: pane.autostart
            onToggled: EngineBridge.perform("autostart.set", String(checked))
        }

        SettingsNote {
            visible: pane.trayAvailable && ShellPreferences.showTrayIcon
            text: L10n.t("At login, Phone Link starts in the system tray.")
        }

        MacSwitch {
            Layout.fillWidth: true
            text: L10n.t("Open the Phone Link window at launch")
            checked: ShellPreferences.showWindowAtLaunch
            onToggled: ShellPreferences.showWindowAtLaunch = checked
        }

        MacSwitch {
            Layout.fillWidth: true
            enabled: pane.trayAvailable
            text: L10n.t("Show Phone Link in the system tray")
            checked: ShellPreferences.showTrayIcon
            onToggled: ShellPreferences.showTrayIcon = checked
        }

        SettingsNote {
            visible: text.length > 0
            text: !pane.trayAvailable ? L10n.t("This desktop has no system tray.")
                : !ShellPreferences.showTrayIcon ? L10n.t("Phone Link keeps running when you close its window. Open it again from your apps.")
                : ""
        }
    }

    SettingsSection {
        title: EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("Phone")

        MacSwitch {
            Layout.fillWidth: true
            text: L10n.t("Share clipboard")
            enabled: pane.settings.clipboardApplies === true
            checked: pane.settings.clipboard === true
            onToggled: EngineBridge.perform("settings.clipboard", String(checked))
        }

        SettingsNote {
            text: pane.settings.clipboardKnown === true && pane.settings.clipboardCapable !== true
                  ? L10n.t("This phone doesn't support clipboard sharing.")
                  : pane.settings.clipboardKnown === true && pane.settings.clipboardApplies !== true
                  ? L10n.t("Clipboard sharing is turned off on this computer.")
                  : L10n.t("Text and images you copy on one device can be pasted on the other.")
        }

        MacSwitch {
            Layout.fillWidth: true
            text: L10n.t("Allow the phone to control this computer")
            enabled: pane.settings.remoteControlApplies === true
            checked: pane.settings.remoteControl === true
            onToggled: EngineBridge.perform("settings.remoteControl", String(checked))
        }

        SettingsNote {
            text: pane.settings.remoteControlKnown !== true
                  ? L10n.t("Your phone can lock, sleep, restart or shut down this computer.")
                  : pane.settings.remoteControlCapable !== true
                  ? L10n.t("This phone doesn't support controlling this computer.")
                  : pane.settings.remoteControlApplies !== true
                  ? L10n.t("Remote control is turned off on this computer.")
                  : pane.settings.remoteControlV2 === true
                  ? L10n.t("Your phone can lock, sleep, restart or shut down this computer without asking first.")
                  : L10n.t("Your phone can lock this computer without asking first.")
        }

        SettingsMediaControlsRows {}

        SettingsNote {
            visible: !pane.phoneSettings
            text: L10n.t("Settings are available once Phone Link is connected.")
        }
    }

    SettingsSection {
        title: L10n.t("Appearance")

        MacSwitch {
            Layout.fillWidth: true
            text: L10n.t("Use phone wallpaper as sidebar background")
            checked: Preferences.wallpaperBackground
            onToggled: Preferences.wallpaperBackground = checked
        }

        SettingsNote {
            text: L10n.t("The phone's wallpaper shows behind the sidebar.")
        }
    }
}
