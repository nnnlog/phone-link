import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

Rectangle {
    id: detail

    property var calls: []
    readonly property var first: calls.length > 0 ? calls[0] : null
    readonly property var control: { EngineBridge.callsRevision; return JSON.parse(EngineBridge.query("callControl", "")) || {} }

    function kindText(kind) {
        switch (kind) {
        case "incoming": return L10n.t("Incoming Call")
        case "outgoing": return L10n.t("Outgoing Call")
        case "missed": return L10n.t("Missed Call")
        case "rejected": return L10n.t("Declined Call")
        case "voicemail": return L10n.t("Voicemail")
        case "blocked": return L10n.t("Blocked Call")
        default: return L10n.t("Call")
        }
    }

    function duration(seconds) {
        const minutes = Math.floor(seconds / 60)
        const rest = seconds % 60
        return minutes + ":" + (rest < 10 ? "0" : "") + rest
    }

    color: Theme.windowBackground

    EmptyState {
        anchors.centerIn: parent
        width: parent.width - 80
        visible: detail.first === null
        title: L10n.t("No Call Selected")
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 26
        visible: detail.first !== null
        spacing: 10

        Avatar {
            Layout.alignment: Qt.AlignHCenter
            Layout.topMargin: 20
            name: detail.first ? detail.first.name || "" : ""
            photo: detail.first ? detail.first.photo || "" : ""
            size: 72
        }

        Text {
            Layout.alignment: Qt.AlignHCenter
            text: detail.first ? detail.first.name || detail.first.number : ""
            color: Theme.text
            font.pixelSize: Theme.titleSize
            font.weight: Font.Bold
        }

        Text {
            Layout.alignment: Qt.AlignHCenter
            visible: detail.first !== null && !!detail.first.name
            text: detail.first ? detail.first.number : ""
            color: Theme.secondaryText
            font.pixelSize: Theme.bodySize
        }

        PillButton {
            Layout.alignment: Qt.AlignHCenter
            Layout.topMargin: 6
            visible: detail.control.enabled === true
            enabled: detail.control.status === "ready"
            prominent: true
            text: L10n.t("Call")
            onClicked: EngineBridge.perform("calls.dial", detail.first.number)
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.topMargin: 16
            implicitHeight: history.implicitHeight + 8
            radius: Theme.moduleRadius
            color: Theme.listBackground
            border.color: Theme.separator

            ColumnLayout {
                id: history
                anchors.fill: parent
                anchors.margins: 4
                spacing: 0

                Repeater {
                    model: detail.calls

                    RowLayout {
                        required property var modelData
                        Layout.fillWidth: true
                        Layout.margins: 10

                        Text {
                            Layout.fillWidth: true
                            text: detail.kindText(modelData.kind)
                            color: modelData.kind === "missed" ? "#ff453a" : Theme.text
                            font.pixelSize: Theme.bodySize
                        }

                        Text {
                            visible: modelData.durationSecs > 0
                            text: detail.duration(modelData.durationSecs)
                            color: Theme.secondaryText
                            font.pixelSize: Theme.subheadSize
                        }

                        Text {
                            text: Dates.detailStamp(modelData.startMs)
                            color: Theme.secondaryText
                            font.pixelSize: Theme.subheadSize
                        }
                    }
                }
            }
        }

        Item {
            Layout.fillHeight: true
        }
    }
}
