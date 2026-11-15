import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import QtQuick.Window
import PhoneLink

Window {
    id: window

    required property string sessionId
    property var session: null
    property var status: null
    property string appName

    readonly property string phoneName: EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("Phone")
    readonly property string name: session && !session.phoneScreen && appName.length > 0 ? appName : phoneName
    readonly property bool swapsSides: session ? (Math.abs(session.rotation) / 90) % 2 === 1 : false
    readonly property var rotatedVideo: session && session.videoSize
        ? (swapsSides ? { width: session.videoSize.height, height: session.videoSize.width } : session.videoSize)
        : null
    readonly property var shownSize: rotatedVideo || (session ? session.deviceSize || session.initialDeviceSize : null)
    readonly property bool navigationBar: session !== null && session.navigationButtons
    readonly property bool takesInput: active && session !== null && session.state === "Connected" && session.videoReady
        && status !== null && status.input
    readonly property int navigationBarHeight: 32
    readonly property int toolbarHeight: 36
    readonly property string audioButton: status && status.audio && session ? session.audioButton : "None"
    readonly property bool offersLayouts: status !== null && status.adaptiveLayout && session !== null
        && !session.phoneScreen && session.state === "Connected"
        && (session.screenLayout === 1 || session.screenLayout === 2)
    readonly property bool offersKeyboard: status !== null && status.input
    readonly property bool toolbar: audioButton !== "None" || offersLayouts || offersKeyboard
    property string dragState: "none"
    property bool penInRange: false

    function send(name, fields) {
        EngineBridge.perform(name, JSON.stringify(Object.assign({ id: sessionId }, fields)))
    }

    function fit() {
        const size = shownSize
        if (!size)
            return
        const available = Math.max(400, Screen.desktopAvailableHeight * 0.75)
        const aspect = size.width / size.height
        const videoHeight = Math.min(available, size.height)
        height = Math.round(videoHeight + (navigationBar ? navigationBarHeight : 0) + (toolbar ? toolbarHeight : 0))
        width = Math.round(Math.max(240, videoHeight * aspect))
    }

    function focusState() {
        if (visibility === Window.Minimized || visibility === Window.Hidden)
            return "suspended"
        return active ? "in" : "out"
    }

    title: name
    minimumWidth: 240
    minimumHeight: 200
    width: 400
    height: 780
    color: "black"
    visible: true

    Component.onCompleted: {
        fit()
        EngineBridge.perform("mirror.ready", sessionId)
        raise()
        requestActivate()
    }
    onShownSizeChanged: fit()
    onActiveChanged: send("mirror.focus", { focus: focusState() })
    onVisibilityChanged: send("mirror.focus", { focus: focusState() })
    onTakesInputChanged: send("mirror.inputFocus", { focused: takesInput })
    function inputMethodActive() {
        const language = Qt.inputMethod.locale.name.split("_")[0]
        return ["ko", "ja", "zh"].includes(language)
    }

    function startPhoneDrag(urls) {
        dragSource.Drag.mimeData = { "text/uri-list": urls.join("\r\n") }
        dragSource.Drag.active = true
        const action = dragSource.Drag.startDrag(Qt.CopyAction)
        dragSource.Drag.active = false
        send("mirror.phoneDragEnded", { dropped: action === Qt.CopyAction })
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

    onClosing: close => {
        close.accepted = true
        EngineBridge.perform("mirror.close", sessionId)
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: window.toolbarHeight
            visible: window.toolbar
            color: Theme.sidebarBackground

            Row {
                anchors.right: parent.right
                anchors.rightMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                spacing: 6

                IconButton {
                    id: volumeButton
                    visible: window.audioButton === "Volume"
                    focusPolicy: Qt.NoFocus
                    iconName: window.session && window.session.muted ? "speakerSlash" : "speaker"
                    ToolTip.visible: hovered
                    ToolTip.delay: 600
                    ToolTip.text: L10n.t("Volume")
                    onClicked: volumePopup.open()

                    Popup {
                        id: volumePopup
                        y: volumeButton.height + 4
                        x: volumeButton.width - width
                        padding: 12
                        onClosed: window.send("mirror.volume", {
                            volume: volumeSlider.value, muted: muteButton.muted
                        })
                        onAboutToShow: {
                            volumeSlider.value = window.session ? window.session.volume : 50
                            muteButton.muted = window.session ? window.session.muted : false
                        }

                        Row {
                            spacing: 8

                            IconButton {
                                id: muteButton
                                property bool muted: false
                                focusPolicy: Qt.NoFocus
                                iconName: muted ? "speakerSlash" : "speaker"
                                ToolTip.visible: hovered
                                ToolTip.delay: 600
                                ToolTip.text: L10n.t(muted ? "Unmute" : "Mute")
                                onClicked: muted = !muted
                            }

                            Slider {
                                id: volumeSlider
                                width: 160
                                anchors.verticalCenter: parent.verticalCenter
                                from: 0
                                to: 100
                                stepSize: 1
                                onMoved: muteButton.muted = false
                            }
                        }
                    }
                }

                IconButton {
                    visible: window.audioButton === "EnableAudio"
                    focusPolicy: Qt.NoFocus
                    iconName: "speakerSlash"
                    ToolTip.visible: hovered
                    ToolTip.delay: 600
                    ToolTip.text: L10n.t("Turn on this app's audio on your phone")
                    onClicked: window.send("mirror.enableAudio", {})
                }

                IconButton {
                    readonly property bool compact: window.session !== null && window.session.screenLayout === 1
                    visible: window.offersLayouts
                    focusPolicy: Qt.NoFocus
                    iconName: compact ? "tablet" : "handset"
                    ToolTip.visible: hovered
                    ToolTip.delay: 600
                    ToolTip.text: L10n.t(compact ? "Switch to Tablet Layout" : "Switch to Phone Layout")
                    onClicked: window.send("mirror.switchLayout", { layout: window.session.screenLayout === 2 ? 1 : 2 })
                }

                IconButton {
                    visible: window.offersKeyboard
                    focusPolicy: Qt.NoFocus
                    iconName: "keyboard"
                    ToolTip.visible: hovered
                    ToolTip.delay: 600
                    ToolTip.text: L10n.t("Choose the keyboard language on your phone")
                    onClicked: window.send("mirror.keyboardSettings", { inputMethod: window.inputMethodActive() })
                }
            }
        }

        Item {
            id: area
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            focus: true

            readonly property real scale: window.rotatedVideo
                ? Math.min(width / window.rotatedVideo.width, height / window.rotatedVideo.height) : 0
            readonly property real shownWidth: window.rotatedVideo ? window.rotatedVideo.width * scale : 0
            readonly property real shownHeight: window.rotatedVideo ? window.rotatedVideo.height * scale : 0
            readonly property real shownX: (width - shownWidth) / 2
            readonly property real shownY: (height - shownHeight) / 2

            function position(x, y) {
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

            Keys.onPressed: event => {
                window.send("mirror.key", { scan: event.nativeScanCode - 8, down: true, repeat: event.isAutoRepeat })
                event.accepted = true
            }
            Keys.onReleased: event => {
                if (!event.isAutoRepeat)
                    window.send("mirror.key", { scan: event.nativeScanCode - 8, down: false })
                event.accepted = true
            }

            MirrorVideo {
                id: video
                anchors.centerIn: parent
                width: window.swapsSides ? area.shownHeight : area.shownWidth
                height: window.swapsSides ? area.shownWidth : area.shownHeight
                rotation: window.session ? window.session.rotation : 0
                sessionId: window.sessionId
                visible: window.rotatedVideo !== null
                Component.onCompleted: attach()
            }

            MouseArea {
                anchors.fill: parent
                hoverEnabled: true
                acceptedButtons: Qt.AllButtons
                enabled: window.takesInput
                onPositionChanged: mouse => {
                    window.send("mirror.mouse", area.position(mouse.x, mouse.y))
                    window.send("mirror.takePhoneDrag", { held: mouse.buttons !== Qt.NoButton })
                }
                onPressed: mouse => {
                    area.forceActiveFocus()
                    window.send("mirror.mouse", Object.assign(area.position(mouse.x, mouse.y),
                        { button: area.buttonName(mouse.button), down: true }))
                }
                onReleased: mouse => window.send("mirror.mouse", Object.assign(area.position(mouse.x, mouse.y),
                    { button: area.buttonName(mouse.button), down: false }))
                onWheel: wheel => window.send("mirror.wheel", Object.assign(area.position(wheel.x, wheel.y),
                    { dx: wheel.angleDelta.x, dy: wheel.angleDelta.y }))
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

            HoverHandler {
                acceptedDevices: PointerDevice.Stylus
                enabled: window.takesInput
                onHoveredChanged: {
                    window.penInRange = hovered
                    if (!hovered)
                        window.send("mirror.pen", Object.assign(area.position(point.position.x, point.position.y),
                            { pressure: 0, rotation: 0, inRange: false, inContact: false }))
                }
                onPointChanged: {
                    if (hovered && !stylus.active)
                        window.send("mirror.pen", Object.assign(area.position(point.position.x, point.position.y),
                            { pressure: 0, rotation: point.rotation, inRange: true, inContact: false }))
                }
            }

            PointHandler {
                id: stylus
                acceptedDevices: PointerDevice.Stylus
                enabled: window.takesInput
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
                    report(active)
                }
                onPointChanged: if (active) report(true)
            }

            DropArea {
                anchors.fill: parent
                keys: ["text/uri-list"]
                onEntered: drag => {
                    window.dragState = "none"
                    window.send("mirror.dragEnter", { urls: drag.urls.map(url => url.toString()) })
                    drag.accepted = true
                }
                onPositionChanged: drag => {
                    window.send("mirror.dragOver", area.position(drag.x, drag.y))
                    drag.accepted = window.dragState === "copy"
                }
                onExited: window.send("mirror.dragLeave", {})
                onDropped: drop => {
                    window.send("mirror.dragDrop", {})
                    if (window.dragState === "copy") {
                        drop.acceptProposedAction()
                    }
                    window.dragState = "none"
                }
            }

            MirrorOverlay {
                anchors.fill: parent
                session: window.session
                name: window.name
                phoneName: window.phoneName
                status: window.status
            }
        }

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
                        { button: "recents", label: "Recent Apps" },
                        { button: "home", label: "Home" },
                        { button: "back", label: "Back" }
                    ]

                    AbstractButton {
                        id: navButton
                        required property var modelData
                        width: 40
                        height: window.navigationBarHeight
                        focusPolicy: Qt.NoFocus
                        hoverEnabled: true
                        ToolTip.visible: hovered
                        ToolTip.delay: 600
                        ToolTip.text: L10n.t(modelData.label)
                        onClicked: window.send("mirror.nav", { button: modelData.button })

                        contentItem: Item {
                            opacity: navButton.down ? 0.5 : 1

                            Rectangle {
                                anchors.centerIn: parent
                                visible: navButton.modelData.button !== "back"
                                width: navButton.modelData.button === "home" ? 13 : 12
                                height: width
                                radius: navButton.modelData.button === "home" ? width / 2 : 2
                                color: "transparent"
                                border.color: Theme.text
                                border.width: 1.5
                            }

                            Icon {
                                anchors.centerIn: parent
                                visible: navButton.modelData.button === "back"
                                name: "back"
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
