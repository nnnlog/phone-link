import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

Rectangle {
    id: view

    readonly property string phase: EngineBridge.phase
    readonly property var phones: JSON.parse(EngineBridge.phonesJson)

    color: Theme.windowBackground

    Loader {
        anchors.fill: parent
        active: view.phase === "signIn"
        sourceComponent: SignInView {}
    }

    ColumnLayout {
        anchors.centerIn: parent
        width: Math.min(420, parent.width - 80)
        visible: view.phase !== "signIn"
        spacing: 14

        BusyIndicator {
            Layout.alignment: Qt.AlignHCenter
            visible: view.phase === "starting" || view.phase === "idle"
            running: visible
        }

        EmptyState {
            Layout.fillWidth: true
            iconName: {
                switch (view.phase) {
                case "selectingPhone": return "device"
                case "failed": return "warning"
                case "accountRemoved": return "person"
                default: return ""
                }
            }
            title: {
                switch (view.phase) {
                case "selectingPhone": return L10n.t("Choose Your Phone")
                case "failed": return L10n.t("Phone Link Couldn't Connect")
                case "accountRemoved": return L10n.t("Sign In Again")
                default: return L10n.t("Connecting to your phone...")
                }
            }
            description: {
                switch (view.phase) {
                case "failed": return EngineBridge.message
                case "accountRemoved": return L10n.t("This computer was removed from your Microsoft account's devices.")
                case "selectingPhone":
                    return view.phones.length === 0
                        ? L10n.t("Link your phone to this Microsoft account with Link to Windows on the phone, then try again.")
                        : ""
                default: return ""
                }
            }
        }

        Rectangle {
            Layout.fillWidth: true
            visible: view.phase === "selectingPhone" && view.phones.length > 0
            implicitHeight: phoneList.implicitHeight + 8
            radius: Theme.moduleRadius
            color: Theme.listBackground
            border.color: Theme.separator

            ColumnLayout {
                id: phoneList
                anchors.fill: parent
                anchors.margins: 4
                spacing: 0

                Repeater {
                    model: view.phones

                    AbstractButton {
                        id: choice
                        required property var modelData
                        Layout.fillWidth: true
                        implicitHeight: 44
                        hoverEnabled: true
                        onClicked: EngineBridge.selectPhone(modelData.id)

                        background: Rectangle {
                            radius: Theme.rowRadius
                            color: choice.hovered ? Theme.hover : "transparent"
                        }

                        contentItem: RowLayout {
                            spacing: 10

                            Icon {
                                Layout.leftMargin: 10
                                name: "device"
                                size: 20
                                color: Theme.accent
                            }

                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 0

                                Text {
                                    text: choice.modelData.name
                                    color: Theme.text
                                    font.pixelSize: Theme.bodySize + 1
                                    font.weight: Font.DemiBold
                                }

                                Text {
                                    visible: !!choice.modelData.model && choice.modelData.model !== choice.modelData.name
                                    text: choice.modelData.model || ""
                                    color: Theme.secondaryText
                                    font.pixelSize: Theme.captionSize
                                }
                            }
                        }
                    }
                }
            }
        }

        PillButton {
            Layout.alignment: Qt.AlignHCenter
            visible: view.phase === "failed" || view.phase === "accountRemoved"
                     || (view.phase === "selectingPhone" && view.phones.length === 0)
            prominent: true
            text: L10n.t(view.phase === "accountRemoved" ? "Sign In" : "Try Again")
            onClicked: EngineBridge.start()
        }
    }
}
