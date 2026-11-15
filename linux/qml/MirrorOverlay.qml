import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

Rectangle {
    id: overlay

    property var session: null
    property string name
    property string phoneName
    property var status: null
    property bool slow: false
    property bool enteringPin: false

    readonly property var shown: session ? describe(session) : null

    function button(title, action, arg) {
        return { title: L10n.t(title), action: action, arg: arg === undefined ? "" : arg }
    }

    function message(icon, title, text, buttons) {
        return { kind: "message", icon: icon, title: title, text: text, buttons: buttons }
    }

    function errorTexts(kind) {
        const n = name, p = phoneName
        switch (kind) {
        case "AppNotAvailable": return [L10n.t("App Not Available"), L10n.f("%@ isn't available on %@.", [n, p])]
        case "Overheated": return [L10n.t("Your Phone Is Too Warm"), L10n.f("Let %@ cool down, then try again.", [p])]
        case "PowerSaving": return [L10n.t("Your Phone Is Saving Power"), L10n.f("Turn off power saving on %@, then try again.", [p])]
        case "LaunchFailed": return [L10n.f("Couldn't Open %@", [n]), L10n.f("%@ couldn't open %@.", [p, n])]
        case "PermissionOff": return [L10n.t("Apps Are Turned Off"), L10n.f("Phone Link isn't allowed to open apps from %@ on this Mac.", [p])]
        case "Disconnected": return [L10n.t("Disconnected"), L10n.f("The connection to %@ ended.", [p])]
        case "TimedOut": return [L10n.t("Your Phone Didn't Respond"), L10n.f("%@ didn't answer in time. Keep it nearby and unlocked, then connect again.", [p])]
        case "CannotConnectDirectly": return [L10n.t("Couldn't Connect"), L10n.f("This Mac couldn't connect to %@. Check that both are online, then try again.", [p])]
        case "PhoneUnreachable": return [L10n.t("Your Phone Can't Be Reached"), L10n.f("Check that %@ is on and online, then connect again.", [p])]
        case "MeteredConnection": return [L10n.t("Your Phone Is on Mobile Data"), L10n.f("%@ doesn't share its screen over a metered connection. Connect it to Wi-Fi, then try again.", [p])]
        case "FeaturesDisabled": return [L10n.t("Link to Windows Is Turned Off"), L10n.f("Turn on Link to Windows on %@, then connect again.", [p])]
        case "UpdateNeeded": return [L10n.t("Update Link to Windows"), L10n.f("Update Link to Windows on %@, then connect again.", [p])]
        case "ServiceIssue": return [L10n.t("Service Unavailable"), L10n.f("The service that connects to %@ didn't respond. Try again later.", [p])]
        default: return [L10n.t("Something Went Wrong"), L10n.f("%@'s screen couldn't be shown.", [p])]
        }
    }

    function errorOverlay(error, dismissible) {
        const texts = errorTexts(error ? error.kind : "Generic")
        let only
        switch (error ? error.action : "Retry") {
        case "Close": only = dismissible ? button("Dismiss", "mirror.dismiss") : button("Close", "mirror.close"); break
        case "Connect": only = button("Connect", "mirror.retry"); break
        case "AllowAndRetry": only = button("Allow and Try Again", "mirror.allowAndRetry"); break
        case "Relaunch": only = button("Open Again", "mirror.reopen"); break
        default: only = button("Try Again", "mirror.retry")
        }
        return message("warning", texts[0], texts[1], [only])
    }

    function describe(s) {
        const n = name, p = phoneName
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
                L10n.f("Allow Link to Windows on %@ to share its screen with this Mac.", [p]),
                [button("Ask Again", "mirror.resendPermission")])
        case "Error":
        case "DismissibleError":
            return errorOverlay(s.error, s.state === "DismissibleError")
        case "Restart":
            return s.restartLayoutSwitch ? { kind: "layoutSwitch" } : { kind: "moveToMac", taskTransfer: s.restartTaskTransfer }
        case "FoldedCannotDraw":
            return message("device", L10n.t("Unfold Your Phone"), L10n.f("%@ can't show %@ while it's folded.", [p, n]), [])
        case "Locked":
            return message("device", L10n.t("Your Phone Is Locked"),
                s.secureLocked ? L10n.f("%@ is locked. Unlock it to continue.", [p]) : L10n.f("%@ is locked.", [p]),
                s.secureLocked ? [] : [button("Unlock", "mirror.wake")])
        case "ScreenOff":
            return message("moon", L10n.t("Your Phone's Screen Is Off"), L10n.f("Wake %@ to continue.", [p]),
                [button("Wake", "mirror.wake")])
        case "Suspended":
            return message("pause", L10n.f("%@ Is Paused", [n]), L10n.f("%@ paused %@ to open other apps.", [p, n]),
                [button("Resume", "mirror.resume")])
        case "Closed":
            return message("close", L10n.f("%@ Was Closed", [n]), L10n.f("%@ was closed on %@.", [n, p]),
                [button("Reopen", "mirror.reopen"), button("Close", "mirror.close")])
        case "TrueTouchWarning":
            return { kind: "trueTouch" }
        case "DeviceOverheat":
            return message("warning", L10n.t("Your Phone Is Too Warm"), L10n.f("Let %@ cool down, then open %@ again.", [p, n]),
                [button("Close", "mirror.close")])
        case "PowerSavingMode":
            return message("warning", L10n.t("Your Phone Is Saving Power"),
                L10n.f("Turn off power saving on %@ to open %@ on this Mac.", [p, n]), [button("Open Again", "mirror.reopen")])
        case "AudioPermissionNeeded":
            return message("speakerSlash", L10n.t("Allow Audio on Your Phone"),
                L10n.f("To hear %@ on this Mac, allow Link to Windows on %@ to share its audio.", [n, p]), [
                    button("Send Request to Phone", "mirror.audioPermission", JSON.stringify({ openSettings: false })),
                    button("Open Phone Settings", "mirror.audioPermission", JSON.stringify({ openSettings: true })),
                    button("Continue Without Audio", "mirror.continueWithoutAudio")
                ])
        case "AudioPermissionAlwaysDenied":
            return message("speakerSlash", L10n.t("Audio Is Turned Off"),
                L10n.f("Link to Windows on %@ isn't allowed to share audio. Allow it in the phone's settings.", [p]), [
                    button("Open Phone Settings", "mirror.audioPermission", JSON.stringify({ openSettings: true })),
                    button("Continue Without Audio", "mirror.continueWithoutAudio")
                ])
        case "SecureContent":
            return message("warning", L10n.t("Protected Content"), L10n.f("%@ is showing something that can't be shown on this Mac.", [n]), [])
        default:
            return null
        }
    }

    function act(action, arg) {
        EngineBridge.perform(action, arg.length > 0 ? arg : session.id)
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
                case "nearby": return "device"
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
                case "trueTouch": return L10n.t("Continue on This Mac?")
                case "moveToMac": return shown.taskTransfer ? L10n.t("Continue on This Mac?") : L10n.f("Open %@ on This Mac?", [overlay.name])
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
            wrapMode: Text.WordWrap
        }

        Text {
            Layout.fillWidth: true
            visible: text.length > 0
            readonly property bool wrongPin: overlay.shown?.kind === "nearby" && overlay.enteringPin && overlay.status?.nearby?.pinWrong === true
            text: {
                const shown = overlay.shown
                const n = overlay.name, p = overlay.phoneName
                if (!shown)
                    return ""
                switch (shown.kind) {
                case "progress": return shown.text
                case "message": return shown.text
                case "trueTouch": return L10n.f("%@ wants to confirm that you're touching %@. It may behave differently when you use it from this Mac.", [n, p])
                case "moveToMac": return shown.taskTransfer
                    ? L10n.f("%@ opens in this window where you left off on %@.", [n, p])
                    : L10n.f("%@ restarts %@ in this window.", [p, n])
                case "layoutSwitch": return L10n.f("%@ restarts on %@ in the new layout.", [n, p])
                case "nearby": {
                    const prompt = overlay.status?.nearby
                    if (!prompt) return L10n.f("Checking that %@ is nearby...", [p])
                    if (prompt.skipUnlock) return L10n.f("Unlock %@, or continue without unlocking it.", [p])
                    if (prompt.skipVerification) return ""
                    if (overlay.enteringPin) return wrongPin
                        ? L10n.f("That code isn't right. Enter the code %@ shows.", [p])
                        : L10n.f("Enter the code %@ shows.", [p])
                    if (prompt.tokenExpired) return L10n.t("The code expired.")
                    return L10n.f("%@ isn't on the same network as this Mac. Verify with a code from your phone.", [p])
                }
                default: return ""
                }
            }
            color: wrongPin ? "#ff453a" : Theme.secondaryText
            font.pixelSize: Theme.bodySize
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
        }

        CheckBox {
            id: doNotAsk
            Layout.alignment: Qt.AlignHCenter
            visible: ["trueTouch", "moveToMac", "layoutSwitch"].includes(overlay.shown?.kind)
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
                submittable: false
                onAccepted: verify.clicked()
            }

            PillButton {
                id: verify
                prominent: true
                text: L10n.t("Verify")
                enabled: pinField.text.trim().length > 0
                onClicked: {
                    EngineBridge.perform("mirror.nearbyPin", pinField.text.trim())
                    pinField.text = ""
                }
            }
        }

        RowLayout {
            Layout.alignment: Qt.AlignHCenter
            Layout.topMargin: 4
            spacing: 8

            Repeater {
                model: overlay.shown?.kind === "message" ? overlay.shown.buttons : []

                PillButton {
                    required property var modelData
                    required property int index
                    prominent: index === 0
                    text: modelData.title
                    onClicked: overlay.act(modelData.action, modelData.arg)
                }
            }

            PillButton {
                visible: overlay.shown?.kind === "trueTouch"
                text: L10n.t("Cancel")
                onClicked: overlay.act("mirror.close", "")
            }

            PillButton {
                visible: overlay.shown?.kind === "trueTouch"
                prominent: true
                text: L10n.t("Continue")
                onClicked: overlay.sessionAct("mirror.trueTouch", { doNotShow: doNotAsk.checked })
            }

            PillButton {
                visible: overlay.shown?.kind === "moveToMac"
                text: L10n.t("Not Now")
                onClicked: overlay.sessionAct("mirror.moveToMac", { accept: false, doNotAsk: doNotAsk.checked })
            }

            PillButton {
                visible: overlay.shown?.kind === "moveToMac"
                prominent: true
                text: L10n.t(overlay.shown?.taskTransfer ? "Continue" : "Open")
                onClicked: overlay.sessionAct("mirror.moveToMac", { accept: true, doNotAsk: doNotAsk.checked })
            }

            PillButton {
                visible: overlay.shown?.kind === "layoutSwitch"
                prominent: true
                text: L10n.t("Change Layout")
                onClicked: overlay.sessionAct("mirror.confirmLayout", { doNotAsk: doNotAsk.checked })
            }

            PillButton {
                readonly property var prompt: overlay.status?.nearby
                visible: overlay.shown?.kind === "nearby" && prompt && (prompt.skipUnlock || prompt.skipVerification)
                prominent: true
                text: L10n.t("Continue")
                onClicked: EngineBridge.perform(prompt.skipUnlock ? "mirror.nearbySkipUnlock" : "mirror.nearbySkipVerification", "")
            }

            PillButton {
                readonly property var prompt: overlay.status?.nearby
                visible: overlay.shown?.kind === "nearby" && prompt && !prompt.skipUnlock && !prompt.skipVerification
                    && !overlay.enteringPin && prompt.tokenExpired
                prominent: true
                text: L10n.t("Show a New Code")
                onClicked: EngineBridge.perform("mirror.nearbyRefreshed", "")
            }
        }

        Text {
            Layout.alignment: Qt.AlignHCenter
            readonly property var prompt: overlay.status?.nearby
            visible: overlay.shown?.kind === "nearby" && prompt && !prompt.skipUnlock && !prompt.skipVerification
                && !overlay.enteringPin
            text: L10n.t("Use a Code from Your Phone Instead")
            color: Theme.accent
            font.pixelSize: Theme.bodySize

            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onClicked: {
                    overlay.enteringPin = true
                    EngineBridge.perform("mirror.nearbyRequestPin", "")
                }
            }
        }
    }
}
