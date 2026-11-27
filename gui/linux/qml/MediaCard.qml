import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import Qt5Compat.GraphicalEffects
import PhoneLink

// What plays on the phone: album art, title, artist and the app, then previous, play/pause and next,
// play/pause the largest. With two or more media apps the app's name opens a menu that picks the one
// the buttons act on. The phone gives no position or length, so there is no progress bar.
Rectangle {
    id: card

    readonly property var media: { EngineBridge.mediaRevision; return JSON.parse(EngineBridge.query("media", "")) || { shown: false } }
    readonly property bool offersPicker: media.offersPicker === true

    visible: media.shown === true
    implicitHeight: content.implicitHeight + 20
    radius: Theme.moduleRadius
    color: Theme.sidebarModule

    Accessible.role: Accessible.Grouping
    Accessible.name: L10n.t("Now playing on your phone")

    ColumnLayout {
        id: content
        anchors.fill: parent
        anchors.leftMargin: Theme.content - Theme.edge
        anchors.rightMargin: Theme.content - Theme.edge
        anchors.topMargin: 10
        anchors.bottomMargin: 10
        spacing: 6

        RowLayout {
            Layout.fillWidth: true
            spacing: 12

            Rectangle {
                Layout.preferredWidth: 56
                Layout.preferredHeight: 56
                radius: 8
                color: Theme.controlBackground
                clip: true
                Accessible.role: Accessible.Graphic
                Accessible.name: L10n.t("Album art")

                Icon {
                    anchors.centerIn: parent
                    visible: art.status !== Image.Ready
                    name: "musicNote"
                    size: 26
                    color: Theme.secondaryText
                }

                Image {
                    id: art
                    anchors.fill: parent
                    source: card.media.art || ""
                    fillMode: Image.PreserveAspectCrop
                    sourceSize.width: 112
                    sourceSize.height: 112
                }
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2

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
                    visible: text.length > 0
                    text: card.media.artist || ""
                    color: Theme.secondaryText
                    font.pixelSize: Theme.subheadSize
                    elide: Text.ElideRight
                }

                // The media app; with two or more, a menu of them.
                AbstractButton {
                    id: source
                    Layout.topMargin: 2
                    Layout.maximumWidth: parent.width
                    implicitWidth: sourceRow.implicitWidth
                    implicitHeight: sourceRow.implicitHeight
                    enabled: card.offersPicker
                    hoverEnabled: card.offersPicker
                    focusPolicy: card.offersPicker ? Qt.StrongFocus : Qt.NoFocus
                    Accessible.role: card.offersPicker ? Accessible.ButtonMenu : Accessible.StaticText
                    Accessible.name: card.media.app || ""
                    Accessible.description: card.offersPicker ? L10n.t("Choose the app to control") : ""
                    ToolTip.visible: card.offersPicker && hovered
                    ToolTip.delay: 600
                    ToolTip.text: L10n.t("Choose the app to control")
                    onClicked: card.openPicker()

                    contentItem: RowLayout {
                        id: sourceRow
                        spacing: 4

                        Item {
                            Layout.preferredWidth: 14
                            Layout.preferredHeight: 14

                            // The app's icon, a white glyph drawn in the secondary text color.
                            Image {
                                id: appGlyph
                                anchors.fill: parent
                                visible: false
                                source: card.media.appIcon || ""
                                sourceSize.width: 28
                                sourceSize.height: 28
                                fillMode: Image.PreserveAspectFit
                            }

                            ColorOverlay {
                                anchors.fill: parent
                                visible: appGlyph.status === Image.Ready
                                source: appGlyph
                                color: Theme.secondaryText
                            }

                            // No icon: a note on a small badge.
                            Rectangle {
                                anchors.fill: parent
                                visible: appGlyph.status !== Image.Ready
                                radius: 3.5
                                color: Theme.secondaryText

                                Icon {
                                    anchors.centerIn: parent
                                    name: "musicNote"
                                    size: 11
                                    color: Theme.dark ? "#1e1e1e" : "white"
                                }
                            }
                        }

                        Text {
                            Layout.fillWidth: true
                            text: card.media.app || ""
                            color: source.hovered ? Theme.text : Theme.secondaryText
                            font.pixelSize: Theme.captionSize
                            elide: Text.ElideRight
                        }

                        Icon {
                            visible: card.offersPicker
                            name: "chevronDown"
                            size: 11
                            color: Theme.secondaryText
                        }
                    }
                }
            }
        }

        // Previous, play/pause and next, centered; a spinner after them while a command is on its way.
        Item {
            Layout.fillWidth: true
            implicitHeight: buttons.implicitHeight

            RowLayout {
                id: buttons
                anchors.horizontalCenter: parent.horizontalCenter
                spacing: 20

                Repeater {
                    model: [
                        { icon: "backward", action: "media.previous", label: L10n.t("Previous Track"), box: 26, size: 16 },
                        card.media.playing ? { icon: "pause", action: "media.playPause", label: L10n.t("Pause"), box: 30, size: 22 }
                                           : { icon: "play", action: "media.playPause", label: L10n.t("Play"), box: 30, size: 22 },
                        { icon: "forward", action: "media.next", label: L10n.t("Next Track"), box: 26, size: 16 }
                    ]

                    AbstractButton {
                        id: control
                        required property var modelData
                        implicitWidth: modelData.box
                        implicitHeight: modelData.box
                        hoverEnabled: true
                        Accessible.role: Accessible.Button
                        Accessible.name: modelData.label
                        ToolTip.visible: hovered
                        ToolTip.delay: 600
                        ToolTip.text: modelData.label
                        onClicked: EngineBridge.perform(modelData.action, "")

                        contentItem: Item {
                            Icon {
                                anchors.centerIn: parent
                                name: control.modelData.icon
                                filled: true
                                size: control.modelData.size
                                color: Theme.text
                                opacity: control.down ? 0.5 : 1
                            }
                        }
                    }
                }
            }

            BusyIndicator {
                anchors.left: buttons.right
                anchors.leftMargin: 12
                anchors.verticalCenter: buttons.verticalCenter
                width: 22
                height: 22
                padding: 2
                visible: running
                running: card.media.commanding === true
            }
        }
    }

    // Opens the app picker under the app's name, or over it when the window ends first.
    function openPicker() {
        const below = source.mapToItem(null, 0, source.height).y + sessionMenu.implicitHeight + 8
        const fits = card.Window.window === null || below <= card.Window.window.height
        sessionMenu.popup(source, 0, fits ? source.height + 4 : -sessionMenu.implicitHeight - 4)
    }

    Menu {
        id: sessionMenu
        padding: 4

        background: Rectangle {
            implicitWidth: 180
            radius: Theme.rowRadius
            color: Theme.raisedBackground
            border.color: Theme.separator
        }

        Instantiator {
            model: card.media.sessions || []

            delegate: MenuItem {
                id: choice
                required property var modelData
                implicitHeight: 28
                leftPadding: 8
                rightPadding: 12
                text: modelData.app
                checkable: true
                checked: modelData.id === card.media.selected
                onTriggered: EngineBridge.perform("media.select", modelData.id)

                indicator: Item {
                    x: choice.leftPadding
                    implicitWidth: 18
                    implicitHeight: choice.height

                    Icon {
                        anchors.verticalCenter: parent.verticalCenter
                        visible: choice.checked
                        name: "checkmark"
                        size: 16
                        color: Theme.text
                    }
                }

                contentItem: Text {
                    leftPadding: 22
                    text: choice.text
                    color: Theme.text
                    font.pixelSize: Theme.bodySize
                    verticalAlignment: Text.AlignVCenter
                    elide: Text.ElideRight
                }

                background: Rectangle {
                    radius: 6
                    color: choice.highlighted ? Theme.hover : "transparent"
                }
            }

            onObjectAdded: (index, object) => sessionMenu.insertItem(index, object)
            onObjectRemoved: (index, object) => sessionMenu.removeItem(object)
        }
    }
}
