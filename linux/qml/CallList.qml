import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

Rectangle {
    id: column

    property var calls: []
    property double selectedId: -1
    readonly property var state: { EngineBridge.callsRevision; return JSON.parse(EngineBridge.query("callsState", "")) || {} }
    readonly property string access: state.access || "unsupported"
    readonly property string phoneName: EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("Phone")

    color: Theme.listBackground

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Text {
            Layout.fillWidth: true
            Layout.preferredHeight: 52
            Layout.leftMargin: 20
            verticalAlignment: Text.AlignVCenter
            text: L10n.t("Calls")
            color: Theme.text
            font.pixelSize: Theme.headlineSize
            font.weight: Font.Bold
        }

        ListView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: column.calls
            leftMargin: 10
            rightMargin: 10
            bottomMargin: 10
            ScrollBar.vertical: ScrollBar {}

            delegate: AbstractButton {
                id: row
                required property var modelData
                readonly property bool missed: modelData.kind === "missed" || modelData.kind === "rejected"
                width: ListView.view.width - 20
                implicitHeight: 52
                hoverEnabled: true
                onClicked: column.selectedId = modelData.id

                background: Rectangle {
                    radius: Theme.rowRadius
                    color: column.selectedId === row.modelData.id ? Theme.selection
                         : row.hovered ? Theme.hover : "transparent"
                }

                contentItem: RowLayout {
                    spacing: 10

                    Avatar {
                        Layout.leftMargin: 6
                        name: row.modelData.name || ""
                        photo: row.modelData.photo || ""
                        size: 34
                    }

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 1

                        Text {
                            Layout.fillWidth: true
                            text: row.modelData.name || row.modelData.number
                            color: row.missed ? "#ff453a" : Theme.text
                            font.pixelSize: Theme.bodySize + 1
                            font.weight: row.modelData.read ? Font.Normal : Font.Bold
                            elide: Text.ElideRight
                        }

                        RowLayout {
                            spacing: 4

                            Icon {
                                name: row.modelData.kind === "outgoing" ? "phoneOut" : "phoneIn"
                                size: 11
                                color: Theme.secondaryText
                            }

                            Text {
                                text: row.modelData.name ? row.modelData.number : ""
                                color: Theme.secondaryText
                                font.pixelSize: Theme.subheadSize
                                elide: Text.ElideRight
                            }
                        }
                    }

                    Text {
                        Layout.rightMargin: 6
                        text: Dates.listStamp(row.modelData.startMs)
                        color: Theme.secondaryText
                        font.pixelSize: Theme.captionSize
                    }
                }
            }

            ColumnLayout {
                anchors.centerIn: parent
                width: parent.width - 40
                visible: column.calls.length === 0
                spacing: 14

                EmptyState {
                    Layout.fillWidth: true
                    iconName: "phone"
                    title: {
                        switch (column.access) {
                        case "consentRequired": return L10n.t("Call History Is Off on This Computer")
                        case "callLogsUnsupported": return L10n.t("Update Link to Windows")
                        case "postNotificationRequired": return L10n.t("Allow Notifications on Your Phone")
                        case "permissionRequired": return L10n.t("Allow Call History on Your Phone")
                        default: return L10n.t("No Recent Calls")
                        }
                    }
                    description: {
                        switch (column.access) {
                        case "consentRequired":
                            return L10n.t("Turn on call history to see your phone's recent calls on this computer.")
                        case "callLogsUnsupported":
                            return L10n.f("To see recent calls, update Link to Windows on %@, then reconnect.", [column.phoneName])
                        case "postNotificationRequired":
                            return L10n.f("Allow Link to Windows to send notifications on %@ to see your recent calls and use other Phone Link features.", [column.phoneName])
                        case "permissionRequired":
                            return L10n.f("Recent calls from %@ appear here after you allow access on the phone.", [column.phoneName])
                        default: return L10n.t("Calls from your phone appear here.")
                        }
                    }
                }

                PillButton {
                    Layout.alignment: Qt.AlignHCenter
                    visible: column.access === "consentRequired" || column.access === "permissionRequired"
                    prominent: true
                    text: L10n.t(column.access === "consentRequired" ? "Turn On" : "Send Request to Phone")
                    onClicked: EngineBridge.perform(column.access === "consentRequired" ? "calls.turnOn" : "calls.requestPermission", "")
                }
            }
        }
    }
}
