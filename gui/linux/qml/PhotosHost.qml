import QtQuick
import PhoneLink

// The windows and app-wide handlers of the photos and apps part. Main.qml keeps one for the app's lifetime,
// whatever the engine's phase. The photo files handed to other applications go when the app quits.
Item {
    // This part's open windows, for Main.qml's Ctrl+W.
    function windows() {
        return []
    }

    Connections {
        target: Qt.application
        function onAboutToQuit() {
            EngineBridge.perform("photos.quit", "")
        }
    }
}
