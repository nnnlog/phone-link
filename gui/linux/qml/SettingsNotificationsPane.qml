import QtQuick
import QtQuick.Layouts
import PhoneLink

// Notifications: whether the phone's notifications show on this computer, how each app's do, and the
// access the phone gives. Every value is the engine's; an app's pop-up shows the user's choice while the
// phone answers it.
SettingsPane {
    id: pane

    // Bumped on the notifications part's `changed` event and when a mute shown here ends.
    property int revision: 0
    readonly property var settings: {
        revision
        EngineBridge.settingsRevision
        return JSON.parse(EngineBridge.query("notifications.settings", "")) || ({ available: false })
    }
    // While the engine is not running (signed out, account removed, another phone being chosen) nothing
    // is read or changed: the engine may still answer for the previous phone.
    readonly property bool ready: settings.available === true && EngineBridge.phase === "running"
    readonly property bool onScreen: visible && Window.window !== null && Window.window.visible
    // The apps, read while the pane shows (a read asks the engine for every app's setting); a failed read
    // keeps the rows shown.
    property var apps: []
    readonly property string phoneName: EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("your phone")

    function reloadApps() {
        if (!onScreen || !ready || settings.appListSupported !== true)
            return
        const rows = JSON.parse(EngineBridge.query("notifications.apps", ""))
        if (rows === null)
            return
        apps = rows
        appModel.sync(rows, app => app["package"])
    }

    // When the earliest mute shown ends (the engine announces no expiry), the apps are read again.
    function scheduleMuteExpiry() {
        const now = Date.now()
        const ends = apps.map(app => app.mutedUntilMs).filter(until => until !== null && until > now)
        if (ends.length === 0) {
            muteExpiry.stop()
            return
        }
        muteExpiry.interval = Math.min(...ends) - now + 1000
        muteExpiry.restart()
    }

    // The phone is asked for its apps' settings while the pane shows (when it reports its apps); they
    // arrive as a settings change.
    function paneShown() {
        if (onScreen && ready)
            EngineBridge.perform("notifications.openSettingsView", "")
        reloadApps()
    }

    onOnScreenChanged: paneShown()
    onReadyChanged: paneShown()
    onSettingsChanged: reloadApps()
    onAppsChanged: scheduleMuteExpiry()
    Component.onCompleted: paneShown()

    Connections {
        target: EngineBridge

        function onAreaEvent(area, name) {
            if (area === "notifications" && name === "changed")
                pane.revision++
        }
    }

    NotificationText {
        id: texts
    }

    KeyedListModel {
        id: appModel
    }

    Timer {
        id: muteExpiry
        onTriggered: pane.revision++
    }

    SettingsNote {
        visible: !pane.ready
        text: L10n.t("Settings are available once Phone Link is connected.")
    }

    SettingsSection {
        title: L10n.t("Notifications")

        MacSwitch {
            Layout.fillWidth: true
            enabled: pane.ready
            text: L10n.t("Show notifications from my phone")
            checked: pane.settings.pcPermission === true
            onToggled: EngineBridge.perform("notifications.setPcPermission", String(checked))
        }

        MacSwitch {
            Layout.fillWidth: true
            enabled: pane.ready && pane.settings.lowBattery !== null
            text: L10n.t("Alert me when the phone's battery is low")
            checked: pane.settings.lowBattery === true
            onToggled: EngineBridge.perform("notifications.setLowBattery", String(checked))
        }

        MacSwitch {
            Layout.fillWidth: true
            enabled: pane.ready && pane.settings.pcPermission === true
            text: L10n.t("Pause apps that send too many notifications")
            checked: pane.settings.throttling === true
            onToggled: EngineBridge.perform("notifications.setThrottling", String(checked))
        }
    }

    // Per-app settings apply only while the phone reports its apps.
    SettingsSection {
        title: L10n.t("Apps")
        visible: pane.ready && pane.settings.appListSupported === true

        Repeater {
            model: appModel

            RowLayout {
                id: appRow

                required property string json
                readonly property var modelData: JSON.parse(json)

                Layout.fillWidth: true
                Layout.minimumHeight: 30
                // Lined up with the switches' text and knobs.
                Layout.leftMargin: 6
                Layout.rightMargin: 6
                spacing: 10

                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 1
                    opacity: pane.settings.pcPermission === true ? 1 : 0.4

                    Text {
                        Layout.fillWidth: true
                        text: appRow.modelData.name
                        color: Theme.text
                        font.pixelSize: Theme.bodySize
                        elide: Text.ElideRight
                    }

                    Text {
                        Layout.fillWidth: true
                        visible: appRow.modelData.mutedUntilMs !== null
                        text: appRow.modelData.mutedUntilMs !== null
                              ? L10n.f("Muted until %@", [texts.until(appRow.modelData.mutedUntilMs)]) : ""
                        color: Theme.secondaryText
                        font.pixelSize: Theme.captionSize
                        elide: Text.ElideRight
                    }
                }

                NotificationChoiceButton {
                    choice: appRow.modelData.choice
                    appName: appRow.modelData.name
                    enabled: pane.settings.pcPermission === true && !appRow.modelData.pending
                    onChosen: choice => EngineBridge.perform("notifications.setAppChoice", JSON.stringify({
                        "package": appRow.modelData["package"], name: appRow.modelData.name, choice: choice
                    }))
                }
            }
        }

        SettingsNote {
            Layout.topMargin: pane.apps.length > 0 ? 4 : 0
            text: L10n.t("Apps appear here after they send a notification. Choosing Off for an app also stops your phone from sending its notifications.")
        }
    }

    SettingsSection {
        title: L10n.f("On %@", [pane.phoneName])
        visible: pane.ready && pane.settings.capable === true

        NotificationAccessRow {
            title: L10n.t("Notification access")
            allowed: pane.settings.devicePermission === true
            pending: pane.settings.requestingAccess === true
            value: pane.settings.lowRamDevice === true && pane.settings.devicePermission !== true ? L10n.t("Not Supported") : ""
            onRequested: EngineBridge.perform("notifications.requestPaneAccess", "")
        }

        NotificationAccessRow {
            visible: pane.settings.sensitivePromptOffered === true
            title: L10n.t("Sensitive content")
            allowed: pane.settings.sensitivePermission === true
            pending: pane.settings.requestingSensitive === true
            onRequested: EngineBridge.perform("notifications.requestSensitiveAccess", "")
        }
    }
}
