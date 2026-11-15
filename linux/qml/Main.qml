import QtQuick
import QtQuick.Controls.Basic
import Qt.labs.platform as Platform
import PhoneLink

ApplicationWindow {
    id: root

    readonly property bool running: EngineBridge.phase === "running"
    readonly property var mirrorSessions: { EngineBridge.mirroringRevision; return running ? JSON.parse(EngineBridge.query("mirror.sessions", "")) || [] : [] }
    readonly property var mirrorStatus: { EngineBridge.mirroringRevision; return running ? JSON.parse(EngineBridge.query("mirror.status", "")) : null }
    readonly property var apps: { EngineBridge.appsRevision; return running ? JSON.parse(EngineBridge.query("apps", "")) || [] : [] }

    function syncMirrorWindows() {
        const ids = mirrorSessions.map(s => s.id)
        for (let index = mirrorWindows.count - 1; index >= 0; index--) {
            if (!ids.includes(mirrorWindows.get(index).sessionId))
                mirrorWindows.remove(index)
        }
        ids.forEach(id => {
            for (let index = 0; index < mirrorWindows.count; index++) {
                if (mirrorWindows.get(index).sessionId === id)
                    return
            }
            mirrorWindows.append({ sessionId: id })
        })
    }

    function present() {
        root.show()
        root.raise()
        root.requestActivate()
    }

    function reportDisplay() {
        const ratio = Screen.devicePixelRatio
        EngineBridge.perform("mirror.display", JSON.stringify({
            width: Screen.width * ratio,
            height: Screen.height * ratio,
            usableWidth: Screen.desktopAvailableWidth * ratio,
            usableHeight: Screen.desktopAvailableHeight * ratio,
            dpi: 96 * ratio
        }))
    }

    title: "Phone Link"
    width: 1100
    height: 720
    minimumWidth: 1000
    minimumHeight: 640
    visible: !Qt.application.arguments.includes("--background")
    color: Theme.windowBackground

    palette.window: Theme.windowBackground
    palette.windowText: Theme.text
    palette.base: Theme.fieldBackground
    palette.text: Theme.text
    palette.button: Theme.controlBackground
    palette.buttonText: Theme.text
    palette.highlight: Theme.accent
    palette.highlightedText: "white"
    palette.placeholderText: Theme.secondaryText

    Component.onCompleted: {
        reportDisplay()
        EngineBridge.start()
        EngineBridge.perform("app.foreground", String(active))
    }
    onActiveChanged: EngineBridge.perform("app.foreground", String(active))
    onMirrorSessionsChanged: syncMirrorWindows()
    onClosing: close => {
        close.accepted = false
        root.hide()
    }

    Platform.SystemTrayIcon {
        visible: true
        icon.name: "phonelink"
        tooltip: "Phone Link"
        onActivated: reason => {
            if (reason !== Platform.SystemTrayIcon.Context)
                root.present()
        }

        menu: Platform.Menu {
            Platform.MenuItem {
                text: L10n.t("Open Phone Link")
                onTriggered: root.present()
            }

            Platform.MenuSeparator {}

            Platform.MenuItem {
                text: L10n.t("Quit Phone Link")
                onTriggered: Qt.quit()
            }
        }
    }

    Connections {
        target: EngineBridge
        function onMirrorRaise(sessionId) {
            for (let index = 0; index < mirrorInstantiator.count; index++) {
                const window = mirrorInstantiator.objectAt(index)
                if (window && window.sessionId === sessionId) {
                    window.show()
                    window.raise()
                    window.requestActivate()
                }
            }
        }
        function onShowSection(section) {
            root.show()
            root.raise()
            root.requestActivate()
            if (section.length > 0 && content.item && content.item.showSection)
                content.item.showSection(section)
        }
    }

    ListModel {
        id: mirrorWindows
    }

    Instantiator {
        id: mirrorInstantiator
        model: mirrorWindows

        MirrorWindow {
            session: root.mirrorSessions.find(s => s.id === sessionId) || null
            status: root.mirrorStatus
            appName: {
                const app = session ? root.apps.find(a => a["package"] === session["package"]) : null
                return app ? app.name : ""
            }
        }
    }

    Loader {
        id: cameraWindow
        readonly property var camera: {
            EngineBridge.settingsRevision
            return EngineBridge.phase === "running" ? JSON.parse(EngineBridge.query("camera", "")) : null
        }
        active: camera !== null && camera.controlWindow === true

        sourceComponent: CameraControlWindow {
            camera: cameraWindow.camera
        }
    }

    Loader {
        id: content
        anchors.fill: parent
        sourceComponent: EngineBridge.phase === "running" ? mainView : stateView
    }

    Component {
        id: mainView
        MainView {
            onSettingsRequested: {
                settingsWindow.show()
                settingsWindow.raise()
                settingsWindow.requestActivate()
            }
        }
    }

    CallBanner {
        anchors.top: parent.top
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.topMargin: 12
        z: 10
    }

    SettingsWindow {
        id: settingsWindow
        visible: false
    }

    Component {
        id: stateView
        EngineStateView {}
    }
}
