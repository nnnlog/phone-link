import QtQuick
import PhoneLink

// The windows and app-wide handlers of the messages part. Main.qml keeps one for the app's lifetime,
// whatever the engine's phase: it opens attachments in their apps, has the section re-read its names
// and photos when the contacts change, and its Bluetooth notice when the window comes forward.
Item {
    id: host

    // This part's open windows, for Main.qml's Ctrl+W.
    function windows() {
        return []
    }

    Connections {
        target: EngineBridge
        function onAreaEvent(area, name, payload) {
            if (area === "messages" && name === "openFile")
                Qt.openUrlExternally(JSON.parse(payload).url)
        }
        function onContactsRevisionChanged() {
            EngineBridge.perform("messages.contactsChanged", "")
        }
    }

    Connections {
        target: host.Window.window
        function onActiveChanged() {
            EngineBridge.perform("messages.windowActive", String(host.Window.window.active))
        }
    }
}
