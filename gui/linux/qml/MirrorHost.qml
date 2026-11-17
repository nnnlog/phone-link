import QtQuick
import PhoneLink

// The windows of screen mirroring and the phone camera: one MirrorWindow per mirroring session the
// engine opened (those the user asked for and those the phone started), and the camera control window
// while the engine asks for it. Main.qml keeps one for the app's lifetime, whatever the engine's phase.
Item {
    id: host

    readonly property bool running: EngineBridge.phase === "running"
    readonly property var mirrorSessions: { EngineBridge.mirroringRevision; return running ? JSON.parse(EngineBridge.query("mirror.sessions", "")) || [] : [] }
    readonly property var mirrorStatus: { EngineBridge.mirroringRevision; return running ? JSON.parse(EngineBridge.query("mirror.status", "")) : null }

    // This part's open windows, for Main.qml's Ctrl+W.
    function windows() {
        const list = [cameraWindow.item]
        for (let index = 0; index < mirrorInstantiator.count; index++)
            list.push(mirrorInstantiator.objectAt(index))
        return list.filter(window => window)
    }

    function mirrorWindow(sessionId) {
        for (let index = 0; index < mirrorInstantiator.count; index++) {
            const window = mirrorInstantiator.objectAt(index)
            if (window && window.sessionId === sessionId)
                return window
        }
        return null
    }

    // The frames of the mirroring windows other than `sessionId`'s, for its cascade.
    function frames(sessionId) {
        const list = []
        for (let index = 0; index < mirrorInstantiator.count; index++) {
            const window = mirrorInstantiator.objectAt(index)
            if (window && window.sessionId !== sessionId && window.visible)
                list.push({ x: window.x, y: window.y, width: window.width, height: window.height })
        }
        return list
    }

    function syncMirrorWindows() {
        const ids = mirrorSessions.map(s => s.id)
        for (let index = mirrorWindows.count - 1; index >= 0; index--) {
            const id = mirrorWindows.get(index).sessionId
            if (ids.includes(id))
                continue
            // The session ended: its window keeps its frame, and goes.
            const window = mirrorWindow(id)
            if (window)
                window.saveFrame()
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

    onMirrorSessionsChanged: syncMirrorWindows()

    Connections {
        target: EngineBridge
        function onMirrorRaise(sessionId) {
            const window = host.mirrorWindow(sessionId)
            if (window) {
                window.show()
                window.raise()
                window.requestActivate()
            }
        }
        // Only the window of a session that changed reads it again.
        function onAreaEvent(area, name, payload) {
            if (area !== "mirror" || name !== "sessionChanged")
                return
            const window = host.mirrorWindow(JSON.parse(payload).id)
            if (window)
                window.sessionRevision += 1
        }
        // A failed action is also shown in a mirroring or camera window that is the active window, where
        // the user acted: the main window may be behind it or hidden.
        function onFailure(title, message) {
            const window = host.windows().find(window => window.active && window.showFailure)
            if (window)
                window.showFailure(title, message)
        }
    }

    ListModel {
        id: mirrorWindows
    }

    Instantiator {
        id: mirrorInstantiator
        model: mirrorWindows

        MirrorWindow {
            status: host.mirrorStatus
            peerFrames: () => host.frames(sessionId)
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
}
