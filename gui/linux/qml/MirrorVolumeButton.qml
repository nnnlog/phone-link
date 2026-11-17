import QtQuick
import QtQuick.Controls.Basic
import PhoneLink

// A mirroring window's volume: a button whose popup holds a mute button and a 0–100 slider for the
// app's volume on the phone.
//
// The state is the volume as last received from the phone (or set by the mute button); the slider
// starts there and goes back there whenever a new state is received. Moving the slider changes nothing
// on the phone until the change is committed — on the slider's release, the release of an arrow key,
// Page Up, Page Down, Home or End, the slider losing the keyboard focus, the popup closing, or half a
// second after the last scroll-wheel notch — and a commit sends the slider's value, muted at 0. A
// dragged or keyed change counts only when the value differs from the last one committed or received;
// scrolling always counts.
// A state received drops the change not yet committed, as does the window closing (`cancel`). The mute
// button sends the received volume with the mute flag turned over, at once.
IconButton {
    id: control

    // The app plays no sound now.
    property bool silent: false
    // The state as last received.
    property int volume: 50
    property bool muted: false

    // A committed volume or a mute change, for the phone.
    signal send(int volume, bool muted)

    property real slider: 50
    property int lastCommitted: 50
    property bool changed: false
    property bool scrolling: false
    // Precise scrolling that has not yet added up to a notch.
    property real notches: 0

    function clamped(value) {
        return Math.min(Math.max(value, 0), 100)
    }

    // A state from the phone: shown, and a change not yet committed dropped. The same state again
    // changes nothing.
    function receive(newVolume, newMuted) {
        if (newVolume === volume && newMuted === muted)
            return
        volume = newVolume
        muted = newMuted
        cancel()
        lastCommitted = newVolume
        slider = newVolume
    }

    // The user dragged the slider, or moved it with a key, to `value`.
    function moved(value) {
        slider = clamped(value)
        if (!scrolling)
            changed = Math.round(slider) !== lastCommitted
    }

    // One scroll-wheel notch, up or down; its change is committed half a second after the last one.
    function scrolled(up) {
        scrolling = true
        slider = clamped(slider + (up ? 2 : -2))
        scrollSettle.restart()
    }

    // Sends the change not yet committed, if any.
    function commit() {
        scrollSettle.stop()
        const pending = changed || scrolling
        changed = false
        scrolling = false
        if (!pending)
            return
        const level = Math.round(slider)
        lastCommitted = level
        send(level, level === 0)
    }

    // Drops the change not yet committed.
    function cancel() {
        scrollSettle.stop()
        changed = false
        scrolling = false
    }

    // The mute button: the received volume with the mute flag turned over, sent at once.
    function toggleMute() {
        const newVolume = volume, newMuted = !muted
        send(newVolume, newMuted)
        receive(newVolume, newMuted)
    }

    focusPolicy: Qt.NoFocus
    iconName: muted || volume === 0 ? "speakerSlash" : silent ? "speakerQuiet" : "speaker"
    label: L10n.t("Volume")
    onClicked: popup.opened ? popup.close() : popup.open()

    Timer {
        id: scrollSettle
        interval: 500
        onTriggered: control.commit()
    }

    Popup {
        id: popup
        y: control.height + 4
        x: control.width - width
        padding: 12
        // A press on the button closes it through the button's own click.
        closePolicy: Popup.CloseOnEscape | Popup.CloseOnPressOutsideParent
        background: Rectangle {
            radius: Theme.moduleRadius
            color: Theme.raisedBackground
            border.color: Theme.separator
        }
        onOpened: volumeSlider.forceActiveFocus()
        onClosed: control.commit()

        Row {
            spacing: 8

            IconButton {
                focusPolicy: Qt.NoFocus
                iconName: control.muted ? "speakerSlash" : "speaker"
                label: control.muted ? L10n.t("Unmute") : L10n.t("Mute")
                onClicked: control.toggleMute()
            }

            Slider {
                id: volumeSlider
                width: 160
                anchors.verticalCenter: parent.verticalCenter
                from: 0
                to: 100
                stepSize: 1
                snapMode: Slider.SnapAlways
                value: control.slider
                focusPolicy: Qt.StrongFocus
                Accessible.name: L10n.t("Volume")
                onMoved: control.moved(value)
                onPressedChanged: {
                    if (!pressed)
                        control.commit()
                }
                onActiveFocusChanged: {
                    if (!activeFocus)
                        control.commit()
                }

                Keys.onPressed: event => {
                    switch (event.key) {
                    case Qt.Key_PageUp: control.moved(control.slider + 10); break
                    case Qt.Key_PageDown: control.moved(control.slider - 10); break
                    case Qt.Key_Home: control.moved(0); break
                    case Qt.Key_End: control.moved(100); break
                    default: return
                    }
                    event.accepted = true
                }
                Keys.onReleased: event => {
                    if (event.isAutoRepeat)
                        return
                    if ([Qt.Key_Left, Qt.Key_Right, Qt.Key_Up, Qt.Key_Down, Qt.Key_PageUp, Qt.Key_PageDown,
                         Qt.Key_Home, Qt.Key_End].includes(event.key))
                        control.commit()
                }

                // Scrolling up (away from the user) raises the volume; a notch moves it by 2.
                WheelHandler {
                    acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad
                    onWheel: event => {
                        if (event.phase === Qt.ScrollMomentum)
                            return
                        control.notches += event.angleDelta.y / 120
                        while (Math.abs(control.notches) >= 1) {
                            control.scrolled(control.notches > 0)
                            control.notches -= control.notches > 0 ? 1 : -1
                        }
                    }
                }
            }
        }
    }
}
