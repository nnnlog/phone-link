import QtCore
import QtQuick
import PhoneLink

// The windows and app-wide handlers of the files and media playback part: the Send Files window
// (`files.showSendWindow` opens it), and the user's choice to show the phone's media in the desktop's
// media controls, which the app's settings keep and the bridge is told whenever the engine starts or the
// media changes. Main.qml keeps one for the app's lifetime, whatever the engine's phase.
Item {
    id: host

    // This part's open windows, for Main.qml's Ctrl+W.
    function windows() {
        return [sendWindow.item].filter(window => window && window.visible)
    }

    function showSendWindow() {
        sendWindow.active = true
        const window = sendWindow.item
        window.show()
        window.raise()
        window.requestActivate()
    }

    // A failure of the Send Files window's request: shown in it while it is open, else in the main window.
    function sendFailure(failure) {
        const window = sendWindow.item
        if (window && window.visible)
            window.showFailure(failure.title, failure.message)
        else
            EngineBridge.failure(failure.title, failure.message)
    }

    function reportSystemControls() {
        EngineBridge.perform("media.systemControls", String(mediaSettings.systemControls))
    }

    Component.onCompleted: Qt.callLater(reportSystemControls)

    Settings {
        id: mediaSettings
        category: "MediaControls"
        property bool systemControls: false
    }

    // Each runs once the bridge's own call that announced it has returned.
    Connections {
        target: EngineBridge
        function onPhaseChanged() {
            Qt.callLater(host.reportSystemControls)
        }
        function onMediaRevisionChanged() {
            Qt.callLater(host.reportSystemControls)
        }
        function onAreaEvent(area, name, payload) {
            if (area === "files" && name === "showSendWindow")
                Qt.callLater(host.showSendWindow)
            else if (area === "files" && name === "sendFailure")
                Qt.callLater(host.sendFailure, JSON.parse(payload))
            else if (area === "media" && name === "systemControls")
                mediaSettings.systemControls = JSON.parse(payload).on === true
        }
    }

    Loader {
        id: sendWindow
        active: false

        sourceComponent: SendFilesWindow {}
    }
}
