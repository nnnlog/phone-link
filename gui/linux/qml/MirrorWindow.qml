import Qt.labs.settings
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import QtQuick.Window
import PhoneLink

// One mirroring window: the phone's screen or an app's, with its input, the overlay its state calls
// for, a short notice when the app keeps its audio from playing here, a toolbar, and a navigation bar
// under the video while the phone navigates by gestures.
//
// It opens in the frame kept for its app's last layout (on X11 at the kept place too), else at three
// quarters of the screen with the video's aspect. It reshapes when the video's aspect changes, and keeps
// its frame when the user stops moving or resizing it, switches its layout or closes it, and when the
// app quits. The video keeps its aspect inside the window, centered.
Window {
    id: window

    required property string sessionId
    property var status: null
    // The frames of the other mirroring windows (set by MirrorHost).
    property var peerFrames: function() { return [] }

    // Bumped when the engine says this session changed, so that it is read again.
    property int sessionRevision: 0
    readonly property var session: {
        EngineBridge.mirroringRevision
        sessionRevision
        return JSON.parse(EngineBridge.query("mirror.session", sessionId))
    }

    // The phone's name; a phone without one is "your phone", and "Your phone" where a title or a sentence
    // starts with it.
    readonly property string phoneName: EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("your phone")
    readonly property string phoneNameAtStart: EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("Your phone")
    // The name the phone gave the app (kept also for apps the list hides), else its package name.
    readonly property string appName: {
        if (!session)
            return ""
        EngineBridge.appsRevision
        const stored = JSON.parse(EngineBridge.query("apps.storedName", session["package"]))
        return typeof stored === "string" && stored.length > 0 ? stored : session["package"]
    }
    readonly property string name: session && !session.phoneScreen ? appName : phoneNameAtStart
    readonly property bool connected: session !== null && session.state === "Connected"
    // Input reaches the phone: its video shows and the phone takes input.
    readonly property bool takesInput: connected && session.videoReady === true && status !== null && status.input === true
    readonly property bool showsBack: session !== null && (session.state === "Connected" || session.state === "SecureContent")
    readonly property bool navigationBar: session !== null && session.navigationButtons === true
    readonly property string audioButton: status && status.audio && session ? session.audioButton : "None"
    readonly property bool offersLayouts: status !== null && status.adaptiveLayout && session !== null
        && !session.phoneScreen && session.state === "Connected"
        && (session.screenLayout === 1 || session.screenLayout === 2)
    readonly property bool offersKeyboard: status !== null && status.input === true
    readonly property int toolbarHeight: 36
    readonly property int navigationBarHeight: 32
    property string dragState: "none"
    property bool penInRange: false

    // The video's size and rotation as the phone sent them last (`transform()`), and as drawn: a change
    // after the first shows behind a fade, and positions map to the layout drawn.
    property var lastTransform: null
    property var shownTransform: null
    readonly property var shownVideo: shownTransform ? turned(shownTransform) : null

    // What the window keeps of its shape (the header above, and mirror/sizing.rs): the app whose frames
    // it keeps (the phone screen's is empty), and the video aspect it was last shaped for, which place()
    // sets from the opening frame.
    property string appPackage
    property bool placed: false
    property bool settingFrame: false
    property real shapedAspect: 0.5125
    property var previous: null
    property var layout: null
    property var lastLayout: null
    property bool barShown: false
    property bool barApplied: false
    property bool barPending: false
    property bool reshapePending: false

    // Back once the window has stayed connected for two seconds; a press before that waits (several
    // make one) and is dropped when the window turns connected again.
    property string lastState: ""
    property bool backSettled: false
    property bool backQueued: false

    // The app stopped letting its audio play here: said for three seconds, which the window's first
    // activation starts over while the notice shows.
    property bool captureAllowed: true
    property bool captureNotice: false
    property bool activatedOnce: false

    function send(name, fields) {
        EngineBridge.perform(name, JSON.stringify(Object.assign({ id: sessionId }, fields)))
    }

    // What follows the session reads it directly: the bindings on it may not have caught up yet when its
    // change is handled.
    function transform() {
        return session && session.videoSize
            ? { width: session.videoSize.width, height: session.videoSize.height, rotation: session.rotation } : null
    }

    // The size the window takes its shape from: the turned video, else the phone's screen.
    function shownSize() {
        const t = transform()
        return t ? turned(t) : (session ? session.deviceSize : null)
    }

    // The size whose aspect a new window opens with: the turned video, else the phone's size it started with.
    function openingSize() {
        const t = transform()
        return t ? turned(t) : (session ? session.initialDeviceSize : null)
    }

    function turned(t) {
        return (Math.abs(t.rotation) / 90) % 2 === 1 ? { width: t.height, height: t.width } : { width: t.width, height: t.height }
    }

    function sameTransform(a, b) {
        return a.width === b.width && a.height === b.height && a.rotation === b.rotation
    }

    // A failed action, while this is the active window.
    function showFailure(title, message) {
        failureToast.show(title, message)
    }

    function frame(request) {
        return JSON.parse(EngineBridge.query("mirror.frame", JSON.stringify(request)))
    }

    // The room a window has on its screen: the screen less the desktop's panels, as far as Qt tells.
    function visibleRoom() {
        return { width: Math.min(Screen.width, Screen.desktopAvailableWidth), height: Math.min(Screen.height, Screen.desktopAvailableHeight) }
    }

    function screens() {
        return Qt.application.screens.map(s => ({ x: s.virtualX, y: s.virtualY, width: s.width, height: s.height }))
    }

    // What the window adds to its video: the toolbar, and the navigation bar while it shows.
    function nonClient(bar) {
        return { width: 0, height: toolbarHeight + ((bar === undefined ? barShown : bar) ? navigationBarHeight : 0) }
    }

    function size() {
        return { width: width, height: height }
    }

    function setSize(target) {
        settingFrame = true
        width = target.width
        height = target.height
        settingFrame = false
    }

    // Only X11 lets a window say where it goes; under Wayland the compositor places it.
    readonly property bool placesItself: Qt.platform.pluginName === "xcb"

    function constrain() {
        const minimum = frame({ op: "minimum", aspect: shapedAspect, visible: visibleRoom(), nonClient: nonClient() })
        if (!minimum)
            return
        minimumWidth = minimum.width
        minimumHeight = minimum.height
    }

    // The key of the frame kept for `layout` (the Expanded layout, 2, has its own; the others share one).
    function frameKey(layout) {
        return "MirrorWindowFrame." + appPackage + (layout === 2 ? ".expanded" : "")
    }

    // The frame kept for `layout`; null when none is kept, or what is kept doesn't read as a frame.
    function keptFrame(layout) {
        const text = frames.value(frameKey(layout), "")
        if (typeof text !== "string" || text.length === 0)
            return null
        try {
            const kept = JSON.parse(text)
            return kept.width > 0 && kept.height > 0 ? kept : null
        } catch (error) {
            return null
        }
    }

    // A new window of the app opens in the frame kept for its last layout, the Expanded layout falling
    // back to the others'.
    function openingFrame(layout) {
        return keptFrame(layout) || (layout === 2 ? keptFrame(0) : null)
    }

    // Keeps the window's frame for the layout it shows, else (none known yet, or the default layout) for
    // the app's last one; not while it is maximized, full screen, minimized or hidden.
    function saveFrame() {
        if (!placed || visibility !== Window.Windowed)
            return
        const shown = layout !== null && layout !== undefined && layout !== 0 ? layout : (lastLayout || 0)
        frames.setValue(frameKey(shown), JSON.stringify({ x: x, y: y, width: width, height: height }))
    }

    function place() {
        appPackage = session ? session["package"] : ""
        layout = session ? session.screenLayout : null
        lastLayout = session ? session.lastScreenLayout : null
        const opening = layout !== null && layout !== undefined ? layout : (lastLayout !== null && lastLayout !== undefined ? lastLayout : 0)
        const kept = openingFrame(opening)
        const restored = kept ? frame({ op: "restore", saved: kept, screens: screens() }) : null
        if (restored) {
            if (placesItself) {
                const all = screens()
                const spot = frame({ op: "cascade", frame: restored, screens: all.map(s => ({ frame: s, visible: s })), peers: peerFrames() })
                x = spot.x
                y = spot.y
            }
            setSize(restored)
            const videoHeight = restored.height - nonClient().height
            if (videoHeight > 0)
                shapedAspect = restored.width / videoHeight
        } else {
            const video = openingSize()
            const answer = frame({ op: "opening", aspect: video ? video.width / video.height : null,
                                   visible: visibleRoom(), nonClient: nonClient() })
            if (answer) {
                setSize(answer)
                shapedAspect = answer.aspect
            }
        }
        constrain()
        placed = true
    }

    // The window is minimized, maximized or full screen: the window system keeps its size.
    function resizedByWindowSystem() {
        return visibility === Window.Minimized || visibility === Window.Maximized || visibility === Window.FullScreen
    }

    // Reshapes the window for the video when its aspect changed; waits while the window is minimized,
    // maximized or full screen.
    function reshapeForVideo() {
        const t = transform()
        const video = t ? turned(t) : null
        if (!video || video.width <= 0 || video.height <= 0)
            return
        if (resizedByWindowSystem()) {
            reshapePending = true
            return
        }
        reshapePending = false

        const shown = shapedAspect
        const ratio = Screen.devicePixelRatio
        const target = frame({
            op: "reshape", content: { width: video.width / ratio, height: video.height / ratio }, shapedFor: shown,
            current: size(), fallbackAspect: shown, previous: previous, visible: visibleRoom(), nonClient: nonClient()
        })
        shapedAspect = video.width / video.height
        constrain()
        if (target) {
            previous = { aspect: shown, width: width, height: height }
            if (target.width !== width || target.height !== height)
                setSize(target)
        }
        applyNavigationBar()
    }

    // The window's height takes in, or gives back, the navigation bar's — not while it is minimized,
    // nor for a bar that shows before the phone's screen size is known, nor when the window already
    // holds it. A maximized or full-screen window keeps its size.
    function applyNavigationBar() {
        if (!barPending && barApplied === barShown)
            return
        if (visibility === Window.Minimized) {
            barPending = true
            return
        }
        let before = barApplied ? navigationBarHeight : 0
        const after = barShown ? navigationBarHeight : 0
        if (after > before) {
            const screen = shownSize()
            if (!screen) {
                barPending = true
                return
            }
            if (frame({ op: "holdsNavigationBar", current: size(), nonClientWithoutBar: nonClient(false), aspect: screen.width / screen.height }))
                before = after
        }
        barPending = false
        barApplied = barShown
        constrain()
        if (after === before || resizedByWindowSystem())
            return
        const target = frame({ op: "heightChanged", current: size(), delta: after - before, aspect: shapedAspect,
                               visible: visibleRoom(), nonClient: nonClient() })
        if (target)
            setSize(target)
    }

    // The user asks for screen layout `target`: the frame kept for it becomes the one a reshape to its
    // aspect returns to, and the window's frame is kept for the layout it shows now.
    function willSwitchLayout(target) {
        const kept = keptFrame(target)
        const room = kept ? kept.height - nonClient().height : 0
        if (kept && room > 0)
            previous = { aspect: kept.width / room, width: kept.width, height: kept.height }
        saveFrame()
    }

    // Applies the session's layout, volume, state, video and navigation bar now and on every change, once
    // the window has its opening frame.
    function follow() {
        if (!session || !placed)
            return
        if (session.screenLayout !== null && session.screenLayout !== undefined)
            layout = session.screenLayout
        lastLayout = session.lastScreenLayout
        volumeButton.receive(session.volume, session.muted)
        noteState(session.state)
        noteCaptureAllowed(session.audioCaptureAllowed !== false)
        followTransform()
        const bar = session.navigationButtons === true
        if (barShown !== bar) {
            barShown = bar
            applyNavigationBar()
        }
        reshapeForVideo()
    }

    function followTransform() {
        const next = transform()
        if (!next) {
            shownTransform = null
            return
        }
        const last = lastTransform
        lastTransform = next
        if (last && !sameTransform(last, next)) {
            fade.restart()
        } else if (!fade.running) {
            shownTransform = next
        }
    }

    // Follows the window's state for Back: a change stops the wait, and turning connected starts it again
    // and drops a waiting press. When the wait ends, a press that waited goes out.
    function noteState(state) {
        if (state === lastState)
            return
        lastState = state
        backSettling.stop()
        if (state !== "Connected")
            return
        backSettled = false
        backQueued = false
        backSettling.start()
    }

    function back() {
        if (!showsBack)
            return
        if (backSettled)
            send("mirror.nav", { button: "back" })
        else
            backQueued = true
    }

    function noteCaptureAllowed(allowed) {
        if (allowed === captureAllowed)
            return
        captureAllowed = allowed
        if (allowed) {
            captureNoticeEnd.stop()
            captureNotice = false
        } else {
            captureNotice = true
            captureNoticeEnd.restart()
        }
    }

    function startPhoneDrag(urls) {
        dragSource.Drag.mimeData = { "text/uri-list": urls.join("\r\n") }
        dragSource.Drag.active = true
        const action = dragSource.Drag.startDrag(Qt.CopyAction)
        dragSource.Drag.active = false
        send("mirror.phoneDragEnded", { dropped: action === Qt.CopyAction })
    }

    title: name
    minimumWidth: 240
    minimumHeight: 200
    width: 400
    height: 780
    color: "black"
    // Shown once it has its opening frame.
    visible: false

    palette.window: Theme.windowBackground
    palette.windowText: Theme.text
    palette.base: Theme.fieldBackground
    palette.text: Theme.text
    palette.button: Theme.controlBackground
    palette.buttonText: Theme.text
    palette.highlight: Theme.accent
    palette.highlightedText: "white"
    palette.placeholderText: Theme.secondaryText
    palette.light: Theme.paletteLight
    palette.midlight: Theme.paletteMidlight
    palette.mid: Theme.paletteMid
    palette.dark: Theme.paletteDark
    palette.shadow: "black"
    palette.toolTipBase: Theme.toolTipBackground
    palette.toolTipText: Theme.text

    // Mirroring reads the primary screen again before the window says it is ready, and once it is framed
    // (its first activation), when a window's title bar can be read.
    Component.onCompleted: {
        place()
        follow()
        show()
        EngineBridge.perform("mirror.display", "")
        EngineBridge.perform("mirror.ready", sessionId)
        if (takesInput)
            send("mirror.inputFocus", { focused: true })
        raise()
        requestActivate()
    }
    onSessionChanged: follow()
    onActiveChanged: {
        send("mirror.focus", { focus: active ? "in" : "out" })
        if (active && !activatedOnce) {
            activatedOnce = true
            EngineBridge.perform("mirror.display", "")
            if (captureNotice)
                captureNoticeEnd.restart()
        }
    }
    onVisibilityChanged: {
        if (window.visibility !== Window.Windowed)
            return
        if (reshapePending)
            reshapeForVideo()
        applyNavigationBar()
    }
    onTakesInputChanged: {
        send("mirror.inputFocus", { focused: takesInput })
        // The overlay's buttons may have held the focus.
        if (takesInput)
            area.forceActiveFocus()
    }
    onXChanged: if (placed && !settingFrame) frameSettle.restart()
    onYChanged: if (placed && !settingFrame) frameSettle.restart()
    onWidthChanged: if (placed && !settingFrame) frameSettle.restart()
    onHeightChanged: if (placed && !settingFrame) frameSettle.restart()

    onClosing: close => {
        close.accepted = true
        volumeButton.cancel()
        saveFrame()
        EngineBridge.perform("mirror.close", sessionId)
    }

    Settings {
        id: frames
        category: "Mirror"
    }

    // The user stopped moving or resizing the window.
    Timer {
        id: frameSettle
        interval: 1000
        onTriggered: window.saveFrame()
    }

    Timer {
        id: backSettling
        interval: 2000
        onTriggered: {
            window.backSettled = true
            if (window.backQueued) {
                window.backQueued = false
                window.send("mirror.nav", { button: "back" })
            }
        }
    }

    Timer {
        id: captureNoticeEnd
        interval: 3000
        onTriggered: window.captureNotice = false
    }

    // The video fades out, stays hidden a moment, takes its new layout, and fades in. A new transform
    // during the fade starts it again from the video's opacity at that moment.
    SequentialAnimation {
        id: fade
        NumberAnimation { target: video; property: "opacity"; to: 0; duration: 125 }
        PauseAnimation { duration: 400 }
        ScriptAction { script: window.shownTransform = window.lastTransform }
        NumberAnimation { target: video; property: "opacity"; to: 1; duration: 125 }
    }

    Connections {
        target: EngineBridge
        function onMirrorDrag(id, state) {
            if (id === window.sessionId)
                window.dragState = state
        }
        function onMirrorPhoneDrag(id, urls) {
            if (id === window.sessionId)
                window.startPhoneDrag(JSON.parse(urls))
        }
    }

    Connections {
        target: Qt.application
        function onAboutToQuit() {
            window.saveFrame()
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: window.toolbarHeight
            color: Theme.sidebarBackground

            IconButton {
                anchors.left: parent.left
                anchors.leftMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                visible: window.showsBack
                focusPolicy: Qt.NoFocus
                iconName: "back"
                label: L10n.t("Back")
                onClicked: window.back()
            }

            Row {
                anchors.right: parent.right
                anchors.rightMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                spacing: 6

                MirrorVolumeButton {
                    id: volumeButton
                    visible: window.audioButton === "Volume"
                    silent: window.session !== null && window.session.silent === true
                    onSend: (volume, muted) => window.send("mirror.volume", { volume: volume, muted: muted })
                }

                IconButton {
                    visible: window.audioButton === "EnableAudio"
                    focusPolicy: Qt.NoFocus
                    iconName: "speakerSlash"
                    label: L10n.t("Turn on this app's audio on your phone")
                    onClicked: window.send("mirror.enableAudio", {})
                }

                IconButton {
                    readonly property bool compact: window.session !== null && window.session.screenLayout === 1
                    visible: window.offersLayouts
                    focusPolicy: Qt.NoFocus
                    iconName: compact ? "tablet" : "handset"
                    label: compact ? L10n.t("Switch to Tablet Layout") : L10n.t("Switch to Phone Layout")
                    onClicked: {
                        const target = window.session.screenLayout === 2 ? 1 : 2
                        window.willSwitchLayout(target)
                        window.send("mirror.switchLayout", { layout: target })
                    }
                }

                IconButton {
                    visible: window.offersKeyboard
                    focusPolicy: Qt.NoFocus
                    iconName: "keyboard"
                    label: L10n.t("Choose the keyboard language on your phone")
                    // Rust asks the input method framework whether a layout or an input method is in use.
                    onClicked: window.send("mirror.keyboardSettings", {})
                }
            }
        }

        Item {
            id: area
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            focus: true

            readonly property real scale: window.shownVideo
                ? Math.min(width / window.shownVideo.width, height / window.shownVideo.height) : 0
            readonly property real shownWidth: window.shownVideo ? window.shownVideo.width * scale : 0
            readonly property real shownHeight: window.shownVideo ? window.shownVideo.height * scale : 0
            readonly property real shownX: (width - shownWidth) / 2
            readonly property real shownY: (height - shownHeight) / 2

            // A point of the area normalized to the video as drawn (0–1 inside it); outside it as a
            // point the engine does not send.
            function position(x, y) {
                if (shownWidth <= 0 || shownHeight <= 0)
                    return { x: -1, y: -1 }
                return { x: (x - shownX) / shownWidth, y: (y - shownY) / shownHeight }
            }

            function buttonName(button) {
                switch (button) {
                case Qt.LeftButton: return "left"
                case Qt.MiddleButton: return "middle"
                case Qt.RightButton: return "right"
                default: return "other"
                }
            }

            // Keys go to the phone while it takes input, their releases always (the engine drops a release
            // whose press it did not send). A key pressed with the Super key is the desktop's. Otherwise
            // Return answers the overlay, and Tab moves between its buttons.
            Keys.onPressed: event => {
                if (!window.takesInput) {
                    if ((event.key === Qt.Key_Return || event.key === Qt.Key_Enter) && overlay.visible) {
                        overlay.defaultAction()
                        event.accepted = true
                    }
                    return
                }
                if (event.modifiers & Qt.MetaModifier)
                    return
                window.send("mirror.key", { scan: event.nativeScanCode - 8, down: true, repeat: event.isAutoRepeat })
                event.accepted = true
            }
            Keys.onReleased: event => {
                if (!event.isAutoRepeat)
                    window.send("mirror.key", { scan: event.nativeScanCode - 8, down: false })
                event.accepted = window.takesInput
            }

            MirrorVideo {
                id: video
                anchors.centerIn: parent
                width: window.shownTransform && (Math.abs(window.shownTransform.rotation) / 90) % 2 === 1 ? area.shownHeight : area.shownWidth
                height: window.shownTransform && (Math.abs(window.shownTransform.rotation) / 90) % 2 === 1 ? area.shownWidth : area.shownHeight
                rotation: window.shownTransform ? window.shownTransform.rotation : 0
                sessionId: window.sessionId
                visible: window.shownVideo !== null
                Component.onCompleted: attach()
            }

            // Every pointer move first asks whether the phone handed a drag over; then it moves the
            // phone's pointer while the phone takes input. A press goes only while the phone takes input,
            // a release always, for the engine to match.
            MouseArea {
                anchors.fill: parent
                hoverEnabled: true
                acceptedButtons: Qt.AllButtons
                onPositionChanged: mouse => {
                    window.send("mirror.takePhoneDrag", { held: mouse.buttons !== Qt.NoButton })
                    if (window.takesInput)
                        window.send("mirror.mouse", area.position(mouse.x, mouse.y))
                }
                onPressed: mouse => {
                    area.forceActiveFocus()
                    if (window.takesInput)
                        window.send("mirror.mouse", Object.assign(area.position(mouse.x, mouse.y),
                            { button: area.buttonName(mouse.button), down: true }))
                }
                onReleased: mouse => window.send("mirror.mouse", Object.assign(area.position(mouse.x, mouse.y),
                    { button: area.buttonName(mouse.button), down: false }))
                // A scroll is one wheel event at the pointer; the momentum after the fingers lift is not
                // sent. Qt's angleDelta.x is positive to the left; the engine's dx is positive to the right.
                onWheel: wheel => {
                    if (!window.takesInput || wheel.phase === Qt.ScrollMomentum)
                        return
                    window.send("mirror.wheel", Object.assign(area.position(wheel.x, wheel.y),
                        { dx: -wheel.angleDelta.x, dy: wheel.angleDelta.y }))
                }
            }

            Item {
                id: dragSource
                Drag.dragType: Drag.None
                Drag.supportedActions: Qt.CopyAction
            }

            MultiPointTouchArea {
                anchors.fill: parent
                enabled: window.takesInput
                mouseEnabled: false

                function report(points, phase) {
                    for (const point of points) {
                        window.send("mirror.touch", Object.assign(area.position(point.x, point.y), {
                            pointer: point.pointId,
                            phase: phase,
                            pressure: point.pressure,
                            orientation: point.rotation,
                            width: point.ellipseDiameters.width,
                            height: point.ellipseDiameters.height
                        }))
                    }
                }

                onPressed: points => {
                    area.forceActiveFocus()
                    report(points, "down")
                }
                onUpdated: points => report(points, "moved")
                onReleased: points => report(points, "up")
                onCanceled: points => report(points, "cancelled")
            }

            // A tablet pen: leaving the tablet's range and lifting off always go out; the rest only while the
            // phone takes input.
            HoverHandler {
                acceptedDevices: PointerDevice.Stylus
                onHoveredChanged: {
                    window.penInRange = hovered
                    if (!hovered)
                        window.send("mirror.pen", Object.assign(area.position(point.position.x, point.position.y),
                            { pressure: 0, rotation: 0, inRange: false, inContact: false }))
                    else if (window.takesInput)
                        window.send("mirror.pen", Object.assign(area.position(point.position.x, point.position.y),
                            { pressure: 0, rotation: 0, inRange: true, inContact: false }))
                }
                onPointChanged: {
                    if (hovered && !stylus.active && window.takesInput)
                        window.send("mirror.pen", Object.assign(area.position(point.position.x, point.position.y),
                            { pressure: 0, rotation: point.rotation, inRange: true, inContact: false }))
                }
            }

            PointHandler {
                id: stylus
                acceptedDevices: PointerDevice.Stylus
                function report(contact) {
                    window.send("mirror.pen", Object.assign(area.position(point.position.x, point.position.y), {
                        pressure: contact ? point.pressure : 0,
                        rotation: point.rotation,
                        inRange: true,
                        inContact: contact
                    }))
                }
                onActiveChanged: {
                    if (active)
                        area.forceActiveFocus()
                    if (!active || window.takesInput)
                        report(active)
                }
                onPointChanged: if (active && window.takesInput) report(true)
            }

            // Files dragged from this computer go to the phone app under the drop while the window is
            // connected; dropping files over the limits shows what the limits are.
            DropArea {
                id: drops
                anchors.fill: parent
                keys: ["text/uri-list"]

                property bool inside: false

                function enter(drag) {
                    window.dragState = "none"
                    inside = true
                    window.send("mirror.dragEnter", { urls: drag.urls.map(url => url.toString()) })
                    if (window.dragState === "accepted")
                        window.send("mirror.dragOver", area.position(drag.x, drag.y))
                }

                onEntered: drag => {
                    inside = false
                    if (!window.connected) {
                        drag.accepted = false
                        return
                    }
                    enter(drag)
                    drag.accepted = window.dragState !== "refused"
                }
                onPositionChanged: drag => {
                    if (inside && !window.connected) {
                        inside = false
                        window.send("mirror.dragLeave", {})
                    } else if (!inside && window.connected) {
                        enter(drag)
                    } else if (inside && window.dragState !== "overLimit" && window.dragState !== "refused") {
                        window.send("mirror.dragOver", area.position(drag.x, drag.y))
                    }
                    drag.accepted = inside && (window.dragState === "copy" || window.dragState === "overLimit")
                }
                onExited: {
                    if (inside)
                        window.send("mirror.dragLeave", {})
                    inside = false
                }
                onDropped: drop => {
                    const state = window.dragState
                    inside = false
                    window.dragState = "none"
                    if (state === "copy") {
                        window.send("mirror.dragDrop", {})
                        drop.acceptProposedAction()
                        return
                    }
                    window.send("mirror.dragLeave", {})
                    drop.accepted = false
                    if (state === "overLimit")
                        EngineBridge.perform("mirror.dragOverLimit", window.sessionId)
                }
            }

            MirrorOverlay {
                id: overlay
                anchors.fill: parent
                session: window.session
                name: window.name
                phoneName: window.phoneName
                phoneNameAtStart: window.phoneNameAtStart
                status: window.status
                onCloseRequested: window.close()
            }

            Rectangle {
                id: captureNoticeBanner
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.top: parent.top
                anchors.topMargin: 12
                width: Math.min(noticeRow.implicitWidth + 24, parent.width - 24)
                height: noticeRow.implicitHeight + 16
                radius: Math.min(height / 2, 16)
                color: Theme.raisedBackground
                border.color: Theme.separator
                opacity: window.captureNotice ? 1 : 0
                visible: opacity > 0
                Accessible.role: Accessible.AlertMessage
                Accessible.name: noticeText.text

                Behavior on opacity {
                    NumberAnimation { duration: 200 }
                }

                RowLayout {
                    id: noticeRow
                    anchors.centerIn: parent
                    width: parent.width - 24
                    spacing: 6

                    Icon {
                        Layout.alignment: Qt.AlignTop
                        Layout.topMargin: 1
                        name: "speakerSlash"
                        size: 14
                        color: Theme.secondaryText
                    }

                    Text {
                        id: noticeText
                        Layout.fillWidth: true
                        text: L10n.t("This app doesn't let its audio play on other devices")
                        color: Theme.text
                        font.pixelSize: Theme.subheadSize
                        wrapMode: Text.Wrap
                    }
                }
            }

            MirrorFailureToast {
                id: failureToast
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.bottom: parent.bottom
                anchors.bottomMargin: 12
            }
        }

        // Recent Apps, Home and Back, each 40 pixels wide, 60 apart (less in a window under 240 wide),
        // centered; they act while the window is connected, and do not take the keyboard focus from the
        // video.
        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: window.navigationBarHeight
            visible: window.navigationBar
            color: Theme.sidebarBackground

            Row {
                anchors.centerIn: parent
                spacing: parent.width >= 240 ? 60 : Math.max(0, (parent.width - 120) / 2)

                Repeater {
                    model: [
                        { button: "recents", label: L10n.t("Recent Apps") },
                        { button: "home", label: L10n.t("Home Screen") },
                        { button: "back", label: L10n.t("Back") }
                    ]

                    AbstractButton {
                        id: navButton
                        required property var modelData
                        width: 40
                        height: window.navigationBarHeight
                        focusPolicy: Qt.NoFocus
                        hoverEnabled: true
                        Accessible.role: Accessible.Button
                        Accessible.name: modelData.label
                        ToolTip.visible: hovered
                        ToolTip.delay: 600
                        ToolTip.text: modelData.label
                        onClicked: {
                            if (window.connected)
                                window.send("mirror.nav", { button: modelData.button })
                        }

                        contentItem: Item {
                            opacity: navButton.down ? 0.5 : 1

                            Rectangle {
                                anchors.centerIn: parent
                                visible: navButton.modelData.button === "home"
                                width: 13
                                height: width
                                radius: width / 2
                                color: "transparent"
                                border.color: Theme.text
                                border.width: 1.5
                            }

                            Icon {
                                anchors.centerIn: parent
                                visible: navButton.modelData.button !== "home"
                                name: navButton.modelData.button === "back" ? "back" : "recents"
                                size: 14
                                color: Theme.text
                            }
                        }
                    }
                }
            }
        }
    }
}
