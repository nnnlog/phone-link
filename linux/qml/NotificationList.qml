import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

Rectangle {
    id: column

    property var notifications: []
    property string selectedKey

    readonly property var rows: groupedRows(notifications)

    function groupedRows(list) {
        const pinned = list.filter(n => n.pinned)
        const rest = list.filter(n => !n.pinned).sort((a, b) => b.timeMs - a.timeMs)
        const result = []
        if (pinned.length > 0) {
            result.push({ header: L10n.t("Pinned") })
            pinned.forEach(n => result.push({ item: n }))
        }
        let group = ""
        rest.forEach(n => {
            const day = Dates.day(n.timeMs)
            if (day !== group) {
                group = day
                result.push({ header: day })
            }
            result.push({ item: n })
        })
        return result
    }

    color: Theme.listBackground

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        RowLayout {
            Layout.fillWidth: true
            Layout.preferredHeight: 52
            Layout.leftMargin: 20
            Layout.rightMargin: 10
            spacing: 8

            Text {
                Layout.fillWidth: true
                text: L10n.t("Notifications")
                color: Theme.text
                font.pixelSize: Theme.headlineSize
                font.weight: Font.Bold
            }

            IconButton {
                iconName: "bellSlash"
                active: EngineBridge.notificationsMuted
                onClicked: EngineBridge.muteNotifications(!EngineBridge.notificationsMuted)
            }

            PillButton {
                text: L10n.t("Clear All")
                enabled: column.notifications.length > 0
                onClicked: EngineBridge.clearNotifications()
            }
        }

        ListView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: column.rows
            leftMargin: 10
            rightMargin: 10
            bottomMargin: 10
            ScrollBar.vertical: ScrollBar {}

            delegate: Loader {
                required property var modelData
                width: ListView.view.width - 20
                sourceComponent: modelData.header !== undefined ? headerRow : itemRow

                Component {
                    id: headerRow

                    Item {
                        implicitHeight: 34

                        Text {
                            anchors.left: parent.left
                            anchors.leftMargin: 6
                            anchors.bottom: parent.bottom
                            anchors.bottomMargin: 6
                            text: modelData.header
                            color: Theme.secondaryText
                            font.pixelSize: Theme.subheadSize
                            font.weight: Font.DemiBold
                        }

                        Rectangle {
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.bottom: parent.bottom
                            height: 1
                            color: Theme.separator
                        }
                    }
                }

                Component {
                    id: itemRow

                    AbstractButton {
                        id: row
                        readonly property var notification: modelData.item
                        readonly property bool selected: column.selectedKey === notification.key
                        implicitHeight: content.implicitHeight + 16
                        hoverEnabled: true
                        onClicked: column.selectedKey = notification.key

                        background: Rectangle {
                            radius: Theme.rowRadius
                            color: row.selected ? Theme.selection : row.hovered ? Theme.hover : "transparent"
                        }

                        contentItem: Item {
                            ColumnLayout {
                                id: content
                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.verticalCenter: parent.verticalCenter
                                anchors.leftMargin: 6
                                anchors.rightMargin: 6
                                spacing: 2

                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 6

                                    AppGlyph {
                                        name: row.notification.app
                                        packageName: row.notification["package"]
                                        size: 16
                                    }

                                    Text {
                                        Layout.fillWidth: true
                                        text: row.notification.app
                                        color: Theme.secondaryText
                                        font.pixelSize: Theme.subheadSize
                                        elide: Text.ElideRight
                                    }

                                    Text {
                                        text: Dates.time(row.notification.timeMs)
                                        color: Theme.secondaryText
                                        font.pixelSize: Theme.captionSize
                                    }
                                }

                                Text {
                                    Layout.fillWidth: true
                                    visible: text.length > 0
                                    text: row.notification.title
                                    color: Theme.text
                                    font.pixelSize: Theme.bodySize + 1
                                    font.weight: Font.Bold
                                    elide: Text.ElideRight
                                }

                                Text {
                                    Layout.fillWidth: true
                                    visible: text.length > 0
                                    text: row.notification.body
                                    color: Theme.secondaryText
                                    font.pixelSize: Theme.subheadSize
                                    wrapMode: Text.Wrap
                                    maximumLineCount: 2
                                    elide: Text.ElideRight
                                }
                            }
                        }
                    }
                }
            }

            EmptyState {
                anchors.centerIn: parent
                width: parent.width - 40
                visible: column.notifications.length === 0
                iconName: "bell"
                title: L10n.t("No Notifications")
            }
        }
    }
}
