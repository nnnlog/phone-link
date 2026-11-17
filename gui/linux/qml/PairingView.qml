import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// Pairing a phone to this Microsoft account: the QR code to scan with the phone, the PIN to confirm or
// type on it, the manual instructions, and what the session waits for. It starts a QR session when it
// appears with none. With `cancellable` it offers a way out (back to the phone list, or closing the
// window), which `cancelled` reports.
ColumnLayout {
    id: view

    property bool cancellable: false

    signal cancelled()

    readonly property var pairing: {
        EngineBridge.pairingRevision
        return JSON.parse(EngineBridge.query("pairing", "")) || { page: "idle" }
    }
    readonly property string page: pairing.page
    // The QR code's image; `null` when the code can't be drawn.
    readonly property var qrSource: page === "qrCode" ? JSON.parse(EngineBridge.query("qr", pairing.code)) : null
    // The clock the expiry counts down on.
    property double now: Date.now()

    function expiryText(expiresAt) {
        const minutes = Math.round((expiresAt * 1000 - now) / 60000)
        return minutes >= 1 ? L10n.f("Expires in %lld min", [minutes]) : L10n.t("Expires in less than a minute")
    }

    spacing: 14

    Component.onCompleted: {
        if (page === "idle")
            EngineBridge.perform("pairing.start", "qrCode")
    }
    onPairingChanged: now = Date.now()

    Timer {
        interval: 15000
        repeat: true
        running: view.visible && (view.page === "qrCode" || view.page === "pin")
        onTriggered: view.now = Date.now()
    }

    component Title: Text {
        Layout.fillWidth: true
        Layout.maximumWidth: 400
        Layout.alignment: Qt.AlignHCenter
        color: Theme.text
        font.pixelSize: Theme.headlineSize + 1
        font.weight: Font.DemiBold
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.Wrap
    }

    component Explanation: Text {
        Layout.fillWidth: true
        Layout.maximumWidth: 400
        Layout.alignment: Qt.AlignHCenter
        color: Theme.secondaryText
        font.pixelSize: Theme.bodySize
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.Wrap
    }

    component Progress: RowLayout {
        property alias text: label.text

        Layout.alignment: Qt.AlignHCenter
        spacing: 6

        BusyIndicator {
            Layout.preferredWidth: 18
            Layout.preferredHeight: 18
            padding: 0
            running: visible
        }

        Text {
            id: label
            color: Theme.secondaryText
            font.pixelSize: Theme.subheadSize
        }
    }

    component Expiry: Text {
        Layout.alignment: Qt.AlignHCenter
        color: Theme.secondaryText
        font.pixelSize: Theme.captionSize
    }

    // A PIN in large spaced digits; struck through once it no longer works.
    component Pin: TextEdit {
        property bool struck: false

        Layout.alignment: Qt.AlignHCenter
        color: struck ? Theme.secondaryText : Theme.text
        font.family: "monospace"
        font.pixelSize: 40
        font.weight: Font.DemiBold
        font.letterSpacing: 6
        font.strikeout: struck
        readOnly: true
        selectByMouse: true
        selectionColor: Theme.accent
        selectedTextColor: "white"
        Accessible.role: Accessible.StaticText
        Accessible.name: L10n.f("PIN %@", [text.split("").join(" ")])
    }

    PillButton {
        Layout.alignment: Qt.AlignHCenter
        visible: view.page === "idle"
        prominent: true
        text: L10n.t("Show QR Code")
        onClicked: EngineBridge.perform("pairing.start", "qrCode")
    }

    Progress {
        visible: view.page === "loading"
        text: L10n.t("Getting a code...")
    }

    // The QR code.
    Title {
        visible: view.page === "qrCode"
        text: L10n.t("Scan the QR Code with Your Phone")
    }

    Explanation {
        visible: view.page === "qrCode"
        text: L10n.t("On your Android phone, open the camera or Link to Windows and scan this code. Then follow the steps on the phone.")
    }

    // Drawn module by module (no smoothing) on white in both appearances, as scanners need.
    Rectangle {
        Layout.alignment: Qt.AlignHCenter
        Layout.preferredWidth: 220
        Layout.preferredHeight: 220
        visible: view.qrSource !== null
        color: "white"
        Accessible.role: Accessible.Graphic
        Accessible.name: L10n.t("QR code for pairing your phone")

        Image {
            anchors.fill: parent
            source: view.qrSource || ""
            smooth: false
            mipmap: false
            fillMode: Image.PreserveAspectFit
        }
    }

    Explanation {
        visible: view.page === "qrCode" && view.qrSource === null
        text: L10n.t("The code can't be shown.")
    }

    Expiry {
        visible: view.page === "qrCode"
        text: visible ? view.expiryText(view.pairing.expiresAt) : ""
    }

    AbstractButton {
        id: manualButton
        Layout.alignment: Qt.AlignHCenter
        visible: view.page === "qrCode"
        hoverEnabled: true
        Accessible.role: Accessible.Link
        Accessible.name: label.text
        onClicked: EngineBridge.perform("pairing.showManual", "")

        contentItem: Text {
            id: label
            text: L10n.t("Pair Manually")
            color: Theme.accent
            font.pixelSize: Theme.bodySize
            font.underline: manualButton.hovered || manualButton.visualFocus
        }

        background: Item {}
    }

    // The manual instructions.
    Title {
        visible: view.page === "manualInstructions"
        text: L10n.t("Pair Manually")
    }

    GridLayout {
        Layout.alignment: Qt.AlignHCenter
        Layout.maximumWidth: 380
        visible: view.page === "manualInstructions"
        columns: 2
        columnSpacing: 8
        rowSpacing: 10

        Repeater {
            model: [
                L10n.t("On your phone, open this link:"),
                view.pairing.signInWith || "",
                L10n.t("Continue here to get a PIN, then enter it in Link to Windows.")
            ]

            delegate: Item {
                required property int index
                required property string modelData

                Layout.columnSpan: 2
                Layout.fillWidth: true
                implicitHeight: step.implicitHeight + (index === 0 ? link.implicitHeight + 6 : 0)

                Text {
                    text: (parent.index + 1) + "."
                    color: Theme.secondaryText
                    font.pixelSize: Theme.bodySize
                }

                Text {
                    id: step
                    anchors.left: parent.left
                    anchors.leftMargin: 22
                    anchors.right: parent.right
                    text: parent.modelData
                    color: Theme.text
                    font.pixelSize: Theme.bodySize
                    wrapMode: Text.Wrap
                }

                TextEdit {
                    id: link
                    anchors.top: step.bottom
                    anchors.topMargin: 6
                    anchors.left: step.left
                    anchors.right: parent.right
                    visible: parent.index === 0
                    text: view.pairing.link || ""
                    color: Theme.text
                    font.family: "monospace"
                    font.pixelSize: Theme.subheadSize
                    wrapMode: TextEdit.WrapAnywhere
                    readOnly: true
                    selectByMouse: true
                    selectionColor: Theme.accent
                    selectedTextColor: "white"
                }
            }
        }
    }

    RowLayout {
        Layout.alignment: Qt.AlignHCenter
        visible: view.page === "manualInstructions"
        spacing: 8

        PillButton {
            text: L10n.t("Use QR Code")
            onClicked: EngineBridge.perform("pairing.backToQr", "")
        }

        PillButton {
            prominent: true
            text: L10n.t("Continue")
            onClicked: EngineBridge.perform("pairing.continueManually", "")
        }
    }

    // A PIN to enter, and one that no longer works.
    Title {
        visible: view.page === "pin" || view.page === "pinProblem"
        text: view.page === "pin"
              ? (view.pairing.pinKind === "challenge" ? L10n.t("Enter This PIN on Your Phone")
                                                      : L10n.t("Enter This PIN in Link to Windows"))
              : (view.pairing.problem === "expired" ? L10n.t("This PIN Expired")
                                                    : L10n.t("Your Phone Entered a Different PIN"))
    }

    Pin {
        visible: view.page === "pin" || view.page === "pinProblem"
        text: view.pairing.code || ""
        struck: view.page === "pinProblem"
    }

    Expiry {
        visible: view.page === "pin"
        text: visible ? view.expiryText(view.pairing.expiresAt) : ""
    }

    Explanation {
        visible: view.page === "pinProblem" && !!view.pairing.message
        text: view.pairing.message || ""
    }

    Progress {
        visible: view.page === "pinProblem" && view.pairing.pinKind === "challenge" && view.pairing.refreshing === true
        text: L10n.t("Getting a new PIN...")
    }

    PillButton {
        Layout.alignment: Qt.AlignHCenter
        visible: view.page === "pinProblem" && !(view.pairing.pinKind === "challenge" && view.pairing.refreshing === true)
        prominent: true
        text: view.pairing.pinKind === "challenge" ? L10n.t("Get a New PIN") : L10n.t("Start Over")
        onClicked: {
            if (view.pairing.pinKind === "challenge")
                EngineBridge.perform("pairing.refreshPin", "")
            else
                EngineBridge.perform("pairing.start", "manualPin")
        }
    }

    // Waiting for the phone.
    Title {
        visible: view.page === "waitingForPermissions"
        text: L10n.t("Finish on Your Phone")
    }

    Explanation {
        visible: view.page === "waitingForPermissions"
        text: L10n.t("Allow the permissions Link to Windows asks for on your phone.")
    }

    Progress {
        visible: view.page === "waitingForPermissions"
        text: L10n.t("Waiting for your phone...")
    }

    Progress {
        visible: view.page === "completing"
        text: L10n.t("Finishing up...")
    }

    // Done, or why not.
    Rectangle {
        Layout.alignment: Qt.AlignHCenter
        Layout.preferredWidth: 44
        Layout.preferredHeight: 44
        visible: view.page === "paired"
        radius: 22
        color: "#30d158"

        Icon {
            anchors.centerIn: parent
            name: "checkmark"
            size: 26
            color: "white"
        }
    }

    Title {
        visible: view.page === "paired"
        text: L10n.t("Your Phone Is Paired")
    }

    Icon {
        Layout.alignment: Qt.AlignHCenter
        visible: view.page === "failed"
        name: "warning"
        size: 36
        color: Theme.secondaryText
    }

    Explanation {
        visible: view.page === "failed"
        text: view.pairing.message || ""
    }

    PillButton {
        Layout.alignment: Qt.AlignHCenter
        visible: view.page === "failed"
        prominent: true
        text: view.pairing.expired === true ? L10n.t("Get a New Code") : L10n.t("Try Again")
        onClicked: EngineBridge.perform("pairing.retry", "")
    }

    PillButton {
        Layout.alignment: Qt.AlignHCenter
        visible: view.cancellable && view.page !== "paired"
        text: L10n.t("Cancel")
        onClicked: view.cancelled()
    }
}
