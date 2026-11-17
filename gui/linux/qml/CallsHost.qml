import QtQuick
import PhoneLink

// The windows of the calls part: the incoming-call window while a call rings that the user has not
// closed (for at most a minute; this computer rings meanwhile), the call window while a call placed or
// answered on this computer goes on (opened again for each new call after the user closed it), and the
// question of a Bluetooth pairing Phone Link runs. Main.qml keeps one for the app's lifetime, whatever
// the engine's phase.
Item {
    id: host

    readonly property bool running: EngineBridge.phase === "running"
    readonly property var control: { EngineBridge.callsRevision; return running ? JSON.parse(EngineBridge.query("callControl", "")) : null }
    readonly property var popupCalls: control !== null && control.popup ? control.popup : []
    // The code of the open Bluetooth pairing question; empty while none is open.
    property string pairingCode: ""

    // This part's open windows, for Main.qml's Ctrl+W.
    function windows() {
        return [incoming.item, callWindow.item, pairingQuestion.item].filter(window => window)
    }

    // The `bluetoothPairing` event opens the question with its code, or, with none, takes it back.
    Connections {
        target: EngineBridge

        function onAreaEvent(area, name, payload) {
            if (area !== "calls" || name !== "bluetoothPairing")
                return
            host.pairingCode = JSON.parse(payload).code || ""
        }
    }

    Loader {
        id: incoming

        active: host.popupCalls.length > 0

        sourceComponent: IncomingCallWindow {
            calls: host.popupCalls
            canMessage: host.control !== null && host.control.canMessage === true
        }
    }

    Loader {
        id: callWindow

        active: host.control !== null && host.control.callWindowOpen === true && !!host.control.primary

        sourceComponent: CallWindow {
            primary: host.control ? host.control.primary : null
            secondary: host.control ? host.control.secondary : null
        }
    }

    Loader {
        id: pairingQuestion

        active: host.pairingCode.length > 0

        sourceComponent: BluetoothPairingWindow {
            code: host.pairingCode
            onAnswered: confirmed => {
                EngineBridge.perform("calls.bluetoothPairingAnswer", confirmed ? "true" : "false")
                host.pairingCode = ""
            }
        }
    }
}
