import QtQuick
import PhoneLink

// *Call* for a number: places the call on the phone when call control is ready; while call control is
// off, switches it on (explaining first, the first time on this computer). Unavailable, with the reason
// as its help, when the number can't be called from here.
CallPill {
    id: root

    property string number
    // Call control's state (query "callControl"); a view that already reads it passes it in.
    property var control: { EngineBridge.callsRevision; return JSON.parse(EngineBridge.query("callControl", "")) || {} }

    readonly property bool offered: control.offered === true
    readonly property bool hasHistory: control.pcConsent === true
    readonly property bool turnedOn: control.enabled === true
    readonly property bool canDial: control.canDial === true

    text: L10n.t("Call")
    iconName: "phone"
    available: number.length > 0 && offered && hasHistory && (!turnedOn || canDial)
    help: number.length === 0 ? L10n.t("No number to call")
        : !hasHistory ? L10n.t("To call from this computer, turn on call history in Settings › Calls")
        : !turnedOn ? L10n.t("Turn on calls on this computer")
        : !canDial ? L10n.t("This computer isn't connected to your phone")
        : L10n.f("Call %@ using your phone", [number])

    onClicked: {
        if (turnedOn && hasHistory)
            EngineBridge.perform("calls.dial", number)
        else if (intro.needed())
            intro.open()
        else
            EngineBridge.perform("calls.enable", "true")
    }

    CallControlIntroDialog {
        id: intro
    }
}
