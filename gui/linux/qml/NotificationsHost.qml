import QtQuick
import Qt.labs.settings
import PhoneLink

// The windows and app-wide handlers of the notifications part. Main.qml keeps one for the app's lifetime,
// whatever the engine's phase.
Item {
    id: host

    // This part's open windows, for Main.qml's Ctrl+W.
    function windows() {
        return []
    }

    // Hands the engine side the apps kept from earlier runs; it answers with every app it then knows.
    function restoreApps() {
        EngineBridge.perform("notifications.restoreKnownApps", memory.knownApps)
    }

    Component.onCompleted: {
        if (EngineBridge.phase === "running")
            restoreApps()
    }

    // Every app each phone's notification list has shown (phone id → package → name), kept between runs:
    // an app set to Off leaves the list, yet stays in the Notifications settings to be turned on again.
    Settings {
        id: memory
        category: "Notifications"
        property string knownApps: "{}"
    }

    Connections {
        target: EngineBridge

        function onPhaseChanged() {
            if (EngineBridge.phase === "running")
                host.restoreApps()
        }

        // A session with the phone starting or ending can change what the phone shares (its access, its
        // capabilities): the notifications part reads its state again.
        function onPhoneConnectedChanged() {
            if (EngineBridge.phase === "running")
                EngineBridge.perform("notifications.reload", "")
        }

        function onAreaEvent(area, name, payload) {
            if (area === "notifications" && name === "knownApps" && payload !== memory.knownApps)
                memory.knownApps = payload
        }
    }
}
