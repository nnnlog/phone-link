import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Dialogs
import QtQuick.Layouts
import PhoneLink

// The composer under a conversation or a new message: why sending needs Bluetooth when it does, the
// draft's images, "+" for more, the text field and the SIM when the phone has two. Return sends,
// Shift+Return (or Alt+Return) starts a new line, Paste takes copied images where images are offered,
// Escape closes an empty new message, and the field grows to five lines before it scrolls.
ColumnLayout {
    id: composer

    property var detail
    property var draft: null
    readonly property var composerState: detail ? detail.composer : null
    readonly property bool editable: composerState !== null && composerState.enabled
    readonly property var images: draft ? draft.images : []
    readonly property bool hasContent: field.text.trim().length > 0 || images.length > 0
    property bool applying: false
    property string draftKey: ""

    function reloadDraft() {
        draft = JSON.parse(EngineBridge.query("messages.draft", ""))
        if (draft === null)
            return
        const switched = (draft.key || "") !== draftKey
        draftKey = draft.key || ""
        if (field.text !== draft.text) {
            applying = true
            field.text = draft.text
            applying = false
            field.cursorPosition = field.length
        } else if (switched) {
            field.cursorPosition = field.length
        }
    }

    function submit() {
        EngineBridge.perform("messages.send", "")
    }

    spacing: 8
    Component.onCompleted: reloadDraft()

    Connections {
        target: EngineBridge
        function onAreaEvent(area, name, payload) {
            if (area !== "messages")
                return
            if (name === "draft")
                composer.reloadDraft()
            else if (name === "pasteText" && field.activeFocus)
                field.paste()
        }
    }

    FontMetrics {
        id: metrics
        font: field.font
    }

    FileDialog {
        id: imageDialog
        title: L10n.t("Add Images")
        fileMode: FileDialog.OpenFiles
        nameFilters: [L10n.t("Image") + " (*.png *.PNG *.jpg *.JPG *.jpeg *.JPEG *.gif *.GIF *.webp *.WEBP *.heic *.HEIC *.heif *.HEIF *.bmp *.BMP *.tif *.TIF *.tiff *.TIFF *.avif *.AVIF)"]
        onAccepted: EngineBridge.perform("messages.addFiles", JSON.stringify(selectedFiles.map(url => url.toString())))
    }

    RowLayout {
        Layout.fillWidth: true
        visible: composer.detail !== null && composer.detail.bluetoothNotice !== null
        spacing: 8

        Icon {
            Layout.alignment: Qt.AlignTop
            Layout.topMargin: 1
            name: "warning"
            size: 15
            color: Theme.secondaryText
        }

        Text {
            Layout.fillWidth: true
            text: composer.detail !== null && composer.detail.bluetoothNotice !== null ? composer.detail.bluetoothNotice : ""
            color: Theme.secondaryText
            font.pixelSize: Theme.subheadSize
            wrapMode: Text.Wrap
        }
    }

    Row {
        Layout.topMargin: 6
        visible: composer.images.length > 0
        spacing: 10

        Repeater {
            model: composer.images

            Item {
                id: thumbnail
                required property var modelData
                width: 56
                height: 56

                Rectangle {
                    anchors.fill: parent
                    radius: 8
                    color: Theme.controlBackground

                    Icon {
                        anchors.centerIn: parent
                        visible: !image.ready
                        name: "photo"
                        size: 20
                        color: Theme.secondaryText
                    }
                }

                MessagesRoundedImage {
                    id: image
                    anchors.fill: parent
                    radius: 8
                    source: thumbnail.modelData.url
                    sourceSize: Qt.size(160, 160)
                    Accessible.role: Accessible.Graphic
                    Accessible.name: thumbnail.modelData.name.length > 0 ? thumbnail.modelData.name : L10n.t("Image")
                }

                AbstractButton {
                    id: removeImage
                    x: parent.width - width + 6
                    y: -6
                    implicitWidth: 20
                    implicitHeight: 20
                    hoverEnabled: true
                    Accessible.role: Accessible.Button
                    Accessible.name: L10n.t("Remove Image")
                    ToolTip.visible: hovered
                    ToolTip.delay: 600
                    ToolTip.text: L10n.t("Remove Image")
                    onClicked: EngineBridge.perform("messages.removeImage", String(thumbnail.modelData.id))

                    background: Rectangle {
                        radius: width / 2
                        color: removeImage.hovered ? Theme.text : Theme.secondaryText
                        border.color: Theme.windowBackground
                        border.width: 1.5
                    }

                    contentItem: Item {
                        Icon {
                            anchors.centerIn: parent
                            name: "close"
                            size: 10
                            color: Theme.windowBackground
                        }
                    }
                }
            }
        }
    }

    RowLayout {
        Layout.fillWidth: true
        spacing: 8

        IconButton {
            Layout.alignment: Qt.AlignBottom
            Layout.bottomMargin: 1
            iconName: "plus"
            label: L10n.t("Add Images")
            enabled: composer.composerState !== null && composer.composerState.canAddImages
            onClicked: imageDialog.open()
        }

        Rectangle {
            Layout.fillWidth: true
            implicitHeight: Math.min(field.implicitHeight, 5 * metrics.lineSpacing + field.topPadding + field.bottomPadding)
            radius: 15
            color: Theme.fieldBackground
            border.color: field.activeFocus ? Theme.accent : Theme.fieldBorder
            border.width: 1
            opacity: composer.editable ? 1 : 0.6

            ScrollView {
                id: scroller
                anchors.fill: parent
                anchors.rightMargin: sendButton.visible ? 32 : 4

                TextArea {
                    id: field
                    leftPadding: 12
                    rightPadding: 4
                    topPadding: 7
                    bottomPadding: 7
                    wrapMode: TextArea.Wrap
                    textFormat: TextEdit.PlainText
                    readOnly: !composer.editable
                    font.pixelSize: Theme.bodySize
                    color: Theme.text
                    placeholderText: composer.composerState !== null ? composer.composerState.placeholder : ""
                    placeholderTextColor: Theme.secondaryText
                    selectionColor: Theme.accent
                    selectedTextColor: "white"
                    background: null
                    Accessible.name: L10n.t("Message")
                    onTextChanged: {
                        if (!composer.applying)
                            EngineBridge.perform("messages.draftText", text)
                    }
                    Keys.onPressed: event => {
                        const returnKey = event.key === Qt.Key_Return || event.key === Qt.Key_Enter
                        if (returnKey && !field.inputMethodComposing) {
                            if (event.modifiers & (Qt.ShiftModifier | Qt.AltModifier))
                                field.insert(field.cursorPosition, "\n")
                            else
                                composer.submit()
                            event.accepted = true
                        } else if (event.matches(StandardKey.Paste) && composer.editable
                                   && composer.composerState !== null && composer.composerState.canAttach) {
                            EngineBridge.perform("messages.paste", "")
                            event.accepted = true
                        } else if (event.key === Qt.Key_Escape && !field.inputMethodComposing
                                   && composer.detail !== null && composer.detail.kind === "newMessage") {
                            event.accepted = !composer.detail.newMessageHasContent
                            EngineBridge.perform("messages.cancelNewMessage", "")
                        }
                    }
                }
            }

            AbstractButton {
                id: sendButton
                anchors.right: parent.right
                anchors.rightMargin: 5
                anchors.bottom: parent.bottom
                anchors.bottomMargin: (Math.min(parent.height, 30) - height) / 2
                width: 22
                height: 22
                visible: composer.editable && composer.hasContent
                hoverEnabled: true
                Accessible.role: Accessible.Button
                Accessible.name: L10n.t("Send")
                ToolTip.visible: hovered
                ToolTip.delay: 600
                ToolTip.text: L10n.t("Send")
                onClicked: composer.submit()

                background: Rectangle {
                    radius: width / 2
                    color: Theme.accent
                    opacity: sendButton.down ? 0.75 : 1
                }

                contentItem: Item {
                    Icon {
                        anchors.centerIn: parent
                        name: "send"
                        size: 14
                        color: "white"
                    }
                }
            }
        }

        AbstractButton {
            id: simButton
            Layout.alignment: Qt.AlignBottom
            Layout.bottomMargin: 1
            visible: composer.detail !== null && composer.detail.sims.length > 0
            implicitHeight: 30
            implicitWidth: simRow.implicitWidth + 20
            hoverEnabled: true
            Accessible.role: Accessible.ButtonMenu
            Accessible.name: L10n.t("SIM to Send From")
            Accessible.description: composer.detail !== null ? composer.detail.simName : ""
            ToolTip.visible: hovered && !simMenu.visible
            ToolTip.delay: 600
            ToolTip.text: L10n.t("SIM to Send From")
            onClicked: simMenu.open()

            background: Rectangle {
                radius: height / 2
                color: simButton.hovered ? Theme.controlHover : Theme.controlBackground
            }

            contentItem: Item {
                RowLayout {
                    id: simRow
                    anchors.centerIn: parent
                    spacing: 4

                    Icon {
                        name: "sim"
                        size: 14
                        color: Theme.text
                    }

                    Text {
                        text: composer.detail !== null ? composer.detail.simName : ""
                        color: Theme.text
                        font.pixelSize: Theme.subheadSize
                    }

                    Icon {
                        name: "chevronDown"
                        size: 12
                        color: Theme.secondaryText
                    }
                }
            }

            // Above the button, their right edges lined up: the composer is at the window's bottom.
            MessagesMenu {
                id: simMenu
                x: simButton.width - width
                y: -height - 4

                Instantiator {
                    model: composer.detail !== null ? composer.detail.sims : []
                    delegate: MenuItem {
                        required property var modelData
                        text: modelData.name
                        checkable: true
                        checked: modelData.active
                        onTriggered: EngineBridge.perform("messages.selectSim", String(modelData.id))
                    }
                    onObjectAdded: (index, object) => simMenu.insertItem(index, object)
                    onObjectRemoved: (index, object) => simMenu.removeItem(object)
                }
            }
        }
    }
}
