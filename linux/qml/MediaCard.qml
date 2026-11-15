import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

Rectangle {
    id: card

    readonly property var media: { EngineBridge.mediaRevision; return JSON.parse(EngineBridge.query("media", "")) || { shown: false } }

    visible: media.shown === true
    implicitHeight: content.implicitHeight + 20
    radius: Theme.moduleRadius
    color: Theme.sidebarModule

    ColumnLayout {
        id: content
        anchors.fill: parent
        anchors.margins: 10
        spacing: 8

        RowLayout {
            Layout.fillWidth: true
            spacing: 10

            Rectangle {
                Layout.preferredWidth: 52
                Layout.preferredHeight: 52
                radius: 8
                color: Theme.controlBackground
                clip: true

                Image {
                    anchors.fill: parent
                    source: card.media.art || ""
                    fillMode: Image.PreserveAspectCrop
                    sourceSize.width: 104
                    sourceSize.height: 104
                }
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 1

                Text {
                    Layout.fillWidth: true
                    text: card.media.title || ""
                    color: Theme.text
                    font.pixelSize: Theme.bodySize
                    font.weight: Font.DemiBold
                    elide: Text.ElideRight
                }

                Text {
                    Layout.fillWidth: true
                    text: card.media.artist || ""
                    color: Theme.secondaryText
                    font.pixelSize: Theme.subheadSize
                    elide: Text.ElideRight
                }

                RowLayout {
                    spacing: 4

                    Image {
                        Layout.preferredWidth: 12
                        Layout.preferredHeight: 12
                        visible: !!card.media.appIcon
                        source: card.media.appIcon || ""
                        sourceSize.width: 24
                        sourceSize.height: 24
                    }

                    Text {
                        text: card.media.app || ""
                        color: Theme.secondaryText
                        font.pixelSize: Theme.captionSize
                    }
                }
            }
        }

        RowLayout {
            Layout.alignment: Qt.AlignHCenter
            spacing: 24
            enabled: card.media.commanding !== true

            Repeater {
                model: [
                    { icon: "backward", action: "media.previous" },
                    { icon: card.media.playing ? "pause" : "play", action: "media.playPause" },
                    { icon: "forward", action: "media.next" }
                ]

                AbstractButton {
                    required property var modelData
                    implicitWidth: 28
                    implicitHeight: 28
                    onClicked: EngineBridge.perform(modelData.action, "")

                    contentItem: Item {
                        Icon {
                            anchors.centerIn: parent
                            name: modelData.icon
                            filled: true
                            size: 18
                            color: Theme.text
                        }
                    }
                }
            }
        }
    }
}
