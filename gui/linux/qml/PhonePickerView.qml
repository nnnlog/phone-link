import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// The account's linked phones, to choose the one Phone Link connects to; shown after sign-in while no
// phone is selected. The other phones stay linked and are not connected. With no linked phone it pairs
// one by QR code at once; with phones, "Pair a New Phone..." pairs another.
ColumnLayout {
    id: picker

    readonly property var phones: {
        EngineBridge.phonesJson
        EngineBridge.pairingRevision
        return JSON.parse(EngineBridge.query("shell.phones", "")) || []
    }
    property string choice: ""
    property bool pairingShown: false

    function detail(phone) {
        const parts = []
        if (phone.model)
            parts.push(phone.model)
        if (phone.lastSeenMs)
            parts.push(L10n.f("Last seen %@", [Dates.listStamp(phone.lastSeenMs)]))
        return parts.join(" · ")
    }

    function connect() {
        if (choice.length > 0)
            EngineBridge.selectPhone(choice)
    }

    spacing: 12

    onPhonesChanged: {
        if (!phones.some(phone => phone.id === choice))
            choice = phones.length === 1 ? phones[0].id : ""
    }
    Component.onCompleted: {
        if (phones.length === 1)
            choice = phones[0].id
    }

    // No linked phone: pair one; or the pairing the user asked for.
    Loader {
        Layout.fillWidth: true
        active: picker.phones.length === 0 || picker.pairingShown
        visible: active

        sourceComponent: Component {
            PairingView {
                cancellable: picker.phones.length > 0
                onCancelled: {
                    EngineBridge.perform("pairing.cancel", "")
                    picker.pairingShown = false
                }
            }
        }
    }

    Rectangle {
        Layout.alignment: Qt.AlignHCenter
        Layout.preferredWidth: 320
        Layout.topMargin: 4
        implicitHeight: 1
        visible: picker.phones.length === 0
        color: Theme.separator
    }

    Text {
        Layout.fillWidth: true
        visible: picker.phones.length === 0
        text: L10n.t("Already linked your phone in Link to Windows?")
        color: Theme.secondaryText
        font.pixelSize: Theme.subheadSize
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.Wrap
    }

    PillButton {
        Layout.alignment: Qt.AlignHCenter
        visible: picker.phones.length === 0
        text: L10n.t("Check Again")
        onClicked: EngineBridge.perform("shell.reloadPhones", "")
    }

    // The linked phones.
    Text {
        Layout.fillWidth: true
        visible: picker.phones.length > 0 && !picker.pairingShown
        text: L10n.t("Choose the phone to use with this computer. You can change it later in Settings.")
        color: Theme.secondaryText
        font.pixelSize: Theme.bodySize
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.Wrap
    }

    Rectangle {
        Layout.fillWidth: true
        visible: picker.phones.length > 0 && !picker.pairingShown
        implicitHeight: Math.min(list.contentHeight, 4 * 48) + 8
        radius: Theme.moduleRadius
        color: Theme.listBackground
        border.color: Theme.separator

        ListView {
            id: list
            anchors.fill: parent
            anchors.margins: 4
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            model: picker.phones
            ScrollBar.vertical: ScrollBar {
                policy: list.contentHeight > list.height ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff
            }

            delegate: AbstractButton {
                id: row

                required property var modelData
                readonly property bool chosen: picker.choice === modelData.id

                width: ListView.view.width
                implicitHeight: 48
                hoverEnabled: true
                activeFocusOnTab: true
                Accessible.role: Accessible.RadioButton
                Accessible.name: modelData.name
                Accessible.description: picker.detail(modelData)
                Accessible.checked: chosen
                onClicked: picker.choice = modelData.id
                onDoubleClicked: {
                    picker.choice = modelData.id
                    picker.connect()
                }
                Keys.onReturnPressed: picker.connect()
                Keys.onEnterPressed: picker.connect()

                background: Rectangle {
                    radius: Theme.rowRadius
                    color: row.chosen ? Theme.accent : row.hovered ? Theme.hover : "transparent"
                    border.color: row.visualFocus && !row.chosen ? Theme.accent : "transparent"
                }

                contentItem: RowLayout {
                    spacing: 10

                    Icon {
                        Layout.leftMargin: 10
                        name: "device"
                        size: 20
                        color: row.chosen ? "white" : Theme.accent
                    }

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 0

                        Text {
                            Layout.fillWidth: true
                            text: row.modelData.name
                            color: row.chosen ? "white" : Theme.text
                            font.pixelSize: Theme.bodySize + 1
                            font.weight: Font.DemiBold
                            elide: Text.ElideRight
                        }

                        Text {
                            Layout.fillWidth: true
                            visible: text.length > 0
                            text: picker.detail(row.modelData)
                            color: row.chosen ? Qt.rgba(1, 1, 1, 0.85) : Theme.secondaryText
                            font.pixelSize: Theme.captionSize
                            elide: Text.ElideRight
                        }
                    }
                }
            }
        }
    }

    RowLayout {
        Layout.alignment: Qt.AlignHCenter
        visible: picker.phones.length > 0 && !picker.pairingShown
        spacing: 8

        PillButton {
            text: L10n.t("Pair a New Phone...")
            onClicked: picker.pairingShown = true
        }

        PillButton {
            prominent: true
            enabled: picker.choice.length > 0
            text: L10n.t("Connect")
            onClicked: picker.connect()
        }
    }
}
