import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// What a mirroring window shows over (or instead of) the phone's video, by the window's state: a
// spinner, or a message with its buttons over the dimmed video; the prominent button is the default,
// which Return presses (`defaultAction`).
Rectangle {
    id: overlay

    property var session: null
    // The window's name (the app's, or the phone's for the phone screen) and the phone's name. A phone
    // without a name is "Your phone" where a title or a sentence starts with it (`name`,
    // `phoneNameAtStart`) and "your phone" within a sentence (`nameInSentence`, `phoneName`).
    property string name
    property string phoneName
    property string phoneNameAtStart
    readonly property string nameInSentence: session && session.phoneScreen ? phoneName : name
    property var status: null
    property bool slow: false
    property bool enteringPin: false

    readonly property var shown: session ? describe(session) : null

    // A button asks to close the window.
    signal closeRequested()

    function button(title, action, arg) {
        return { title: title, action: action, arg: arg === undefined ? "" : arg }
    }

    function message(icon, title, text, buttons) {
        return { kind: "message", icon: icon, title: title, text: text, buttons: buttons }
    }

    function errorTexts(kind) {
        const n = nameInSentence, p = phoneName
        switch (kind) {
        case "AppNotAvailable": return [L10n.t("App Not Available"), L10n.f("%@ isn't available on %@.", [name, p])]
        case "Overheated": return [L10n.t("Your Phone Is Too Warm"), L10n.f("Let %@ cool down, then try again.", [p])]
        case "PowerSaving": return [L10n.t("Your Phone Is Saving Power"), L10n.f("Turn off power saving on %@, then try again.", [p])]
        case "LaunchFailed": return [L10n.f("Couldn't Open %@", [name]), L10n.f("%@ couldn't open %@.", [phoneNameAtStart, n])]
        case "PermissionOff": return [L10n.t("Apps Are Turned Off"), L10n.f("Phone Link isn't allowed to open apps from %@ on this computer.", [p])]
        case "Disconnected": return [L10n.t("Disconnected"), L10n.f("The connection to %@ ended.", [p])]
        case "TimedOut": return [L10n.t("Your Phone Didn't Respond"), L10n.f("%@ didn't answer in time. Keep it nearby and unlocked, then connect again.", [phoneNameAtStart])]
        case "CannotConnectDirectly": return [L10n.t("Couldn't Connect"), L10n.f("This computer couldn't connect to %@. Check that both are online, then try again.", [p])]
        case "PhoneUnreachable": return [L10n.t("Your Phone Can't Be Reached"), L10n.f("Check that %@ is on and online, then connect again.", [p])]
        case "MeteredConnection": return [L10n.t("Your Phone Is on Mobile Data"), L10n.f("%@ doesn't share its screen over a metered connection. Connect it to Wi-Fi, then try again.", [phoneNameAtStart])]
        case "FeaturesDisabled": return [L10n.t("Link to Windows Is Turned Off"), L10n.f("Turn on Link to Windows on %@, then connect again.", [p])]
        case "UpdateNeeded": return [L10n.t("Update Link to Windows"), L10n.f("Update Link to Windows on %@, then connect again.", [p])]
        case "ServiceIssue": return [L10n.t("Service Unavailable"), L10n.f("The service that connects to %@ didn't respond. Try again later.", [p])]
        default: return [L10n.t("Something Went Wrong"), L10n.f("%@'s screen couldn't be shown.", [phoneNameAtStart])]
        }
    }

    function errorOverlay(error, dismissible) {
        const texts = errorTexts(error ? error.kind : "Generic")
        let only
        switch (error ? error.action : "Retry") {
        case "Close": only = dismissible ? button(L10n.t("Dismiss"), "mirror.dismiss") : button(L10n.t("Close"), "mirror.close"); break
        case "Connect": only = button(L10n.t("Connect"), "mirror.retry"); break
        case "AllowAndRetry": only = button(L10n.t("Allow and Try Again"), "mirror.allowAndRetry"); break
        case "Relaunch": only = button(L10n.t("Open Again"), "mirror.reopen"); break
        default: only = button(L10n.t("Try Again"), "mirror.retry")
        }
        return message("warning", texts[0], texts[1], [only])
    }

    function describe(s) {
        const n = nameInSentence, p = phoneName
        switch (s.state) {
        case "Loading":
            return { kind: "progress", text: slow
                ? L10n.f("This is taking longer than usual. Keep %@ unlocked and nearby.", [p])
                : s.phoneScreen ? L10n.f("Connecting to %@...", [p]) : L10n.f("Opening %@...", [n]) }
        case "Connected":
            return s.videoReady ? null : { kind: "progress", text: L10n.f("Starting %@...", [n]) }
        case "ConnectionUnstable":
            return { kind: "progress", text: L10n.f("Reconnecting to %@...", [p]) }
        case "NearbyConnectionNeeded":
            return { kind: "nearby" }
        case "ScreenCapturePermissionNeeded":
            return message("device", L10n.t("Check Your Phone"),
                L10n.f("Allow Link to Windows on %@ to share its screen with this computer.", [p]),
                [button(L10n.t("Ask Again"), "mirror.resendPermission")])
        case "Error":
        case "DismissibleError":
            return errorOverlay(s.error, s.state === "DismissibleError")
        case "Restart":
            return s.restartLayoutSwitch ? { kind: "layoutSwitch" } : { kind: "moveToComputer", taskTransfer: s.restartTaskTransfer }
        case "FoldedCannotDraw":
            return message("device", L10n.t("Unfold Your Phone"), L10n.f("%@ can't show %@ while it's folded.", [phoneNameAtStart, n]), [])
        case "Locked":
            // A phone behind its PIN, pattern or biometrics unlocks only on the phone itself.
            return message("lock", L10n.t("Your Phone Is Locked"),
                s.secureLocked ? L10n.f("%@ is locked. Unlock it to continue.", [phoneNameAtStart]) : L10n.f("%@ is locked.", [phoneNameAtStart]),
                s.secureLocked ? [] : [button(L10n.t("Unlock"), "mirror.wake")])
        case "ScreenOff":
            return message("moon", L10n.t("Your Phone's Screen Is Off"), L10n.f("Wake %@ to continue.", [p]),
                [button(L10n.t("Wake"), "mirror.wake")])
        case "Suspended":
            return message("pauseCircle", L10n.f("%@ Is Paused", [name]), L10n.f("%@ paused %@ to open other apps.", [phoneNameAtStart, n]),
                [button(L10n.t("Resume"), "mirror.resume")])
        case "Closed":
            return message("appClose", L10n.f("%@ Was Closed", [name]), L10n.f("%@ was closed on %@.", [name, p]),
                [button(L10n.t("Reopen"), "mirror.reopen"), button(L10n.t("Close"), "mirror.close")])
        case "TrueTouchWarning":
            return { kind: "trueTouch" }
        case "DeviceOverheat":
            return message("thermometer", L10n.t("Your Phone Is Too Warm"), L10n.f("Let %@ cool down, then open %@ again.", [p, n]),
                [button(L10n.t("Close"), "mirror.close")])
        case "PowerSavingMode":
            return message("batteryLow", L10n.t("Your Phone Is Saving Power"),
                L10n.f("Turn off power saving on %@ to open %@ on this computer.", [p, n]), [button(L10n.t("Open Again"), "mirror.reopen")])
        case "AudioPermissionNeeded":
            return message("speaker", L10n.t("Allow Audio on Your Phone"),
                L10n.f("To hear %@ on this computer, allow Link to Windows on %@ to share its audio.", [n, p]), [
                    button(L10n.t("Send Request to Phone"), "mirror.audioPermission", JSON.stringify({ openSettings: false })),
                    button(L10n.t("Open Phone Settings"), "mirror.audioPermission", JSON.stringify({ openSettings: true })),
                    button(L10n.t("Continue Without Audio"), "mirror.continueWithoutAudio")
                ])
        case "AudioPermissionAlwaysDenied":
            return message("speakerSlash", L10n.t("Audio Is Turned Off"),
                L10n.f("Link to Windows on %@ isn't allowed to share audio. Allow it in the phone's settings.", [p]), [
                    button(L10n.t("Open Phone Settings"), "mirror.audioPermission", JSON.stringify({ openSettings: true })),
                    button(L10n.t("Continue Without Audio"), "mirror.continueWithoutAudio")
                ])
        case "SecureContent":
            return message("eyeSlash", L10n.t("Protected Content"), L10n.f("%@ is showing something that can't be shown on this computer.", [name]), [])
        default:
            return null
        }
    }

    function act(action, arg) {
        if (action === "mirror.close") {
            closeRequested()
            return
        }
        EngineBridge.perform(action, arg.length > 0 ? arg : session.id)
    }

    // Return: the overlay's default button.
    function defaultAction() {
        const kind = shown ? shown.kind : ""
        const prompt = status ? status.nearby : null
        switch (kind) {
        case "message":
            if (shown.buttons.length > 0)
                act(shown.buttons[0].action, shown.buttons[0].arg)
            break
        case "trueTouch":
            sessionAct("mirror.trueTouch", { doNotShow: doNotAsk.checked })
            break
        case "moveToComputer":
            sessionAct("mirror.moveToComputer", { accept: true, doNotAsk: doNotAsk.checked })
            break
        case "layoutSwitch":
            sessionAct("mirror.confirmLayout", { doNotAsk: doNotAsk.checked })
            break
        case "nearby":
            if (!prompt)
                break
            if (prompt.skipUnlock)
                EngineBridge.perform("mirror.nearbySkipUnlock", "")
            else if (prompt.skipVerification)
                EngineBridge.perform("mirror.nearbySkipVerification", "")
            else if (enteringPin)
                verify.clicked()
            else if (prompt.tokenExpired)
                EngineBridge.perform("mirror.nearbyRefreshed", "")
            break
        }
    }

    function sessionAct(action, fields) {
        EngineBridge.perform(action, JSON.stringify(Object.assign({ id: session.id }, fields)))
    }

    visible: shown !== null
    color: Qt.rgba(Theme.windowBackground.r, Theme.windowBackground.g, Theme.windowBackground.b, 0.92)

    onShownChanged: {
        if (!shown || shown.kind !== "nearby")
            enteringPin = false
    }

    Timer {
        running: overlay.session !== null && overlay.session.state === "Loading"
        interval: 15000
        onRunningChanged: overlay.slow = false
        onTriggered: overlay.slow = true
    }

    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.AllButtons
        onWheel: wheel => wheel.accepted = true
    }

    ColumnLayout {
        id: content
        anchors.centerIn: parent
        width: Math.min(parent.width - 48, 360)
        spacing: 12

        BusyIndicator {
            Layout.alignment: Qt.AlignHCenter
            visible: overlay.shown !== null && (overlay.shown.kind === "progress"
                || (overlay.shown.kind === "nearby" && !overlay.status?.nearby))
            running: visible
        }

        Icon {
            Layout.alignment: Qt.AlignHCenter
            visible: overlay.shown !== null && overlay.shown.kind !== "progress"
                && !(overlay.shown.kind === "nearby" && !overlay.status?.nearby)
            name: {
                switch (overlay.shown?.kind) {
                case "message": return overlay.shown.icon
                case "nearby": return "phoneLock"
                case "trueTouch": return "handRaised"
                case "moveToComputer": return "computerPhone"
                case "layoutSwitch": return "layoutSwitch"
                default: return "device"
                }
            }
            size: 36
            color: Theme.secondaryText
        }

        Text {
            id: titleText
            Layout.fillWidth: true
            visible: text.length > 0
            text: {
                const shown = overlay.shown
                if (!shown)
                    return ""
                switch (shown.kind) {
                case "message": return shown.title
                case "trueTouch": return L10n.t("Continue on This Computer?")
                case "moveToComputer": return shown.taskTransfer ? L10n.t("Continue on This Computer?") : L10n.f("Open %@ on This Computer?", [overlay.name])
                case "layoutSwitch": return L10n.t("Change the Layout?")
                case "nearby": {
                    const prompt = overlay.status?.nearby
                    if (!prompt) return ""
                    if (prompt.skipUnlock) return L10n.t("Unlock Your Phone")
                    if (prompt.skipVerification) return L10n.t("Your Phone Is Verified")
                    return L10n.t("Verify It's You")
                }
                default: return ""
                }
            }
            color: Theme.text
            font.pixelSize: Theme.headlineSize
            font.weight: Font.DemiBold
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
        }

        Text {
            Layout.fillWidth: true
            visible: text.length > 0
            readonly property bool wrongPin: overlay.shown?.kind === "nearby" && overlay.enteringPin && overlay.status?.nearby?.pinWrong === true
            text: {
                const shown = overlay.shown
                const n = overlay.nameInSentence, p = overlay.phoneName
                if (!shown)
                    return ""
                switch (shown.kind) {
                case "progress": return shown.text
                case "message": return shown.text
                case "trueTouch": return L10n.f("%@ wants to confirm that you're touching %@. It may behave differently when you use it from this computer.", [overlay.name, p])
                case "moveToComputer": return shown.taskTransfer
                    ? L10n.f("%@ opens in this window where you left off on %@.", [overlay.name, p])
                    : L10n.f("%@ restarts %@ in this window.", [overlay.phoneNameAtStart, n])
                case "layoutSwitch": return L10n.f("%@ restarts on %@ in the new layout.", [overlay.name, p])
                case "nearby": {
                    const prompt = overlay.status?.nearby
                    if (!prompt) return L10n.f("Checking that %@ is nearby...", [p])
                    if (prompt.skipUnlock) return L10n.f("Unlock %@, or continue without unlocking it.", [p])
                    if (prompt.skipVerification) return ""
                    if (overlay.enteringPin) return wrongPin
                        ? L10n.f("That code isn't right. Enter the code %@ shows.", [p])
                        : L10n.f("Enter the code %@ shows.", [p])
                    if (prompt.tokenExpired) return L10n.t("The code expired.")
                    return L10n.f("%@ isn't on the same network as this computer. Verify with a code from your phone.", [overlay.phoneNameAtStart])
                }
                default: return ""
                }
            }
            color: wrongPin ? Theme.destructive : Theme.secondaryText
            font.pixelSize: Theme.bodySize
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
        }

        CheckBox {
            id: doNotAsk
            Layout.alignment: Qt.AlignHCenter
            visible: ["trueTouch", "moveToComputer", "layoutSwitch"].includes(overlay.shown?.kind)
            text: L10n.t("Don't ask again")
            checked: false
        }

        RowLayout {
            Layout.alignment: Qt.AlignHCenter
            visible: overlay.shown?.kind === "nearby" && overlay.enteringPin
            spacing: 8

            CapsuleField {
                id: pinField
                Layout.preferredWidth: 120
                placeholderText: L10n.t("Code")
                Accessible.name: L10n.t("Code")
                submittable: false
                onAccepted: verify.clicked()
            }

            PillButton {
                id: verify
                prominent: true
                text: L10n.t("Verify")
                enabled: pinField.text.trim().length > 0
                onClicked: {
                    if (pinField.text.trim().length === 0)
                        return
                    EngineBridge.perform("mirror.nearbyPin", pinField.text.trim())
                    pinField.text = ""
                }
            }
        }

        // One row while the buttons fit the column's width, else one full-width button per line.
        GridLayout {
            id: actions
            readonly property var shownButtons: Array.from(children).filter(item => item.visible && item.implicitWidth > 0)
            readonly property real rowWidth: shownButtons.reduce((sum, item) => sum + item.implicitWidth, 0)
                + columnSpacing * Math.max(0, shownButtons.length - 1)
            readonly property bool stacked: rowWidth > content.width
            Layout.alignment: Qt.AlignHCenter
            Layout.fillWidth: stacked
            Layout.topMargin: 4
            columns: stacked ? 1 : Math.max(1, shownButtons.length)
            columnSpacing: 8
            rowSpacing: 8

            Repeater {
                model: overlay.shown?.kind === "message" ? overlay.shown.buttons : []

                PillButton {
                    required property var modelData
                    required property int index
                    Layout.fillWidth: actions.stacked
                    prominent: index === 0
                    text: modelData.title
                    onClicked: overlay.act(modelData.action, modelData.arg)
                }
            }

            PillButton {
                Layout.fillWidth: actions.stacked
                visible: overlay.shown?.kind === "trueTouch"
                text: L10n.t("Cancel")
                onClicked: overlay.closeRequested()
            }

            PillButton {
                Layout.fillWidth: actions.stacked
                visible: overlay.shown?.kind === "trueTouch"
                prominent: true
                text: L10n.t("Continue")
                onClicked: overlay.sessionAct("mirror.trueTouch", { doNotShow: doNotAsk.checked })
            }

            PillButton {
                Layout.fillWidth: actions.stacked
                visible: overlay.shown?.kind === "moveToComputer"
                text: L10n.t("Not Now")
                onClicked: overlay.sessionAct("mirror.moveToComputer", { accept: false, doNotAsk: doNotAsk.checked })
            }

            PillButton {
                Layout.fillWidth: actions.stacked
                visible: overlay.shown?.kind === "moveToComputer"
                prominent: true
                text: overlay.shown?.taskTransfer ? L10n.t("Continue") : L10n.t("Open")
                onClicked: overlay.sessionAct("mirror.moveToComputer", { accept: true, doNotAsk: doNotAsk.checked })
            }

            PillButton {
                Layout.fillWidth: actions.stacked
                visible: overlay.shown?.kind === "layoutSwitch"
                prominent: true
                text: L10n.t("Change Layout")
                onClicked: overlay.sessionAct("mirror.confirmLayout", { doNotAsk: doNotAsk.checked })
            }

            PillButton {
                readonly property var prompt: overlay.status?.nearby
                Layout.fillWidth: actions.stacked
                visible: overlay.shown?.kind === "nearby" && prompt && (prompt.skipUnlock || prompt.skipVerification)
                prominent: true
                text: L10n.t("Continue")
                onClicked: EngineBridge.perform(prompt.skipUnlock ? "mirror.nearbySkipUnlock" : "mirror.nearbySkipVerification", "")
            }

            PillButton {
                readonly property var prompt: overlay.status?.nearby
                Layout.fillWidth: actions.stacked
                visible: overlay.shown?.kind === "nearby" && prompt && !prompt.skipUnlock && !prompt.skipVerification
                    && !overlay.enteringPin && prompt.tokenExpired
                prominent: true
                text: L10n.t("Show a New Code")
                onClicked: EngineBridge.perform("mirror.nearbyRefreshed", "")
            }
        }

        // A link, reached with Tab like the buttons.
        AbstractButton {
            id: useCode
            Layout.alignment: Qt.AlignHCenter
            Layout.maximumWidth: content.width
            readonly property var prompt: overlay.status?.nearby
            visible: overlay.shown?.kind === "nearby" && prompt && !prompt.skipUnlock && !prompt.skipVerification
                && !overlay.enteringPin
            hoverEnabled: true
            padding: 2
            text: L10n.t("Use a Code from Your Phone Instead")
            Accessible.role: Accessible.Link
            Accessible.name: text
            onClicked: {
                overlay.enteringPin = true
                EngineBridge.perform("mirror.nearbyRequestPin", "")
                // The code field shows in its place: typing goes there.
                Qt.callLater(() => pinField.forceActiveFocus())
            }

            HoverHandler {
                cursorShape: Qt.PointingHandCursor
            }

            background: Rectangle {
                radius: 3
                color: "transparent"
                border.color: useCode.visualFocus ? Theme.accent : "transparent"
            }

            contentItem: Text {
                text: useCode.text
                color: Theme.accent
                font.pixelSize: Theme.bodySize
                font.underline: useCode.hovered
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
            }
        }
    }
}
