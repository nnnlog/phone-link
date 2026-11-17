import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// One attachment of a message: an image inline once its picture is on this computer (its row until
// then), or a row saying what it is with what can be done — download it, or open it in its app.
// A right-click on an attachment this computer holds offers Open and Save.
Item {
    id: partView

    property var part

    // Asks the transcript for a file name to save the attachment under.
    signal saveRequested(string key, string fileName)

    readonly property bool showsImage: part.state === "image" && part.url !== null
    readonly property real imageScale: picture.pictureWidth > 0 && picture.pictureHeight > 0
        ? Math.min(240 / picture.pictureWidth, 320 / picture.pictureHeight, 1) : 1

    function kindTitle(kind) {
        switch (kind) {
        case "image": return L10n.t("Image")
        case "video": return L10n.t("Video")
        case "audio": return L10n.t("Audio")
        case "contact": return L10n.t("Contact")
        case "text": return L10n.t("Text")
        default: return L10n.t("Attachment")
        }
    }

    function kindIcon(kind) {
        switch (kind) {
        case "image": return "photo"
        case "video": return "film"
        case "audio": return "waveform"
        case "contact": return "contactCard"
        case "text": return "docText"
        default: return "doc"
        }
    }

    function open() {
        EngineBridge.perform("messages.openPart", part.key)
    }

    implicitWidth: showsImage ? Math.max(1, picture.pictureWidth * imageScale) : part.state === "unsupported"
                   ? unsupported.implicitWidth : attachmentRow.implicitWidth
    implicitHeight: showsImage ? Math.max(1, picture.pictureHeight * imageScale) : part.state === "unsupported"
                    ? unsupported.implicitHeight : attachmentRow.implicitHeight

    TapHandler {
        acceptedButtons: Qt.RightButton
        enabled: partView.part.hasBytes
        onTapped: eventPoint => partMenu.popup(eventPoint.position.x, eventPoint.position.y)
    }

    MessagesMenu {
        id: partMenu

        MenuItem {
            text: L10n.t("Open")
            onTriggered: partView.open()
        }

        MenuItem {
            text: L10n.t("Save As...")
            onTriggered: partView.saveRequested(partView.part.key, partView.part.fileName)
        }
    }

    MessagesRoundedImage {
        id: picture
        anchors.fill: parent
        visible: partView.showsImage
        source: partView.showsImage ? partView.part.url : ""
        sourceSize: Qt.size(640, 640)
        fillMode: Image.PreserveAspectFit
        Accessible.role: Accessible.Graphic
        Accessible.name: L10n.t("Image")

        TapHandler {
            onDoubleTapped: partView.open()
        }
    }

    Rectangle {
        id: unsupported
        visible: partView.part.state === "unsupported"
        implicitWidth: unsupportedText.implicitWidth + 24
        implicitHeight: unsupportedText.implicitHeight + 14
        radius: 16
        color: Theme.controlBackground

        Text {
            id: unsupportedText
            anchors.centerIn: parent
            text: L10n.t("Attachment not supported")
            color: Theme.secondaryText
            font.pixelSize: Theme.bodySize
        }
    }

    Rectangle {
        id: attachmentRow
        visible: !partView.showsImage && partView.part.state !== "unsupported"
        implicitWidth: rowContent.implicitWidth + 24
        implicitHeight: Math.max(rowContent.implicitHeight, 26) + 16
        radius: 16
        color: Theme.controlBackground

        RowLayout {
            id: rowContent
            anchors.centerIn: parent
            spacing: 10

            Icon {
                name: partView.kindIcon(partView.part.kind)
                size: 16
                color: Theme.secondaryText
            }

            Text {
                text: partView.part.size > 0
                      ? partView.kindTitle(partView.part.kind) + " · "
                        + Qt.locale().formattedDataSize(partView.part.size, partView.part.size >= 1000000 ? 1 : 0,
                                                      Locale.DataSizeSIFormat)
                      : partView.kindTitle(partView.part.kind)
                color: Theme.text
                font.pixelSize: Theme.bodySize
            }

            BusyIndicator {
                Layout.preferredWidth: 24
                Layout.preferredHeight: 24
                padding: 2
                visible: partView.part.state === "downloadable" && partView.part.downloading
                running: visible
            }

            PillButton {
                visible: partView.part.state === "downloadable" && !partView.part.downloading
                text: L10n.t("Download")
                onClicked: EngineBridge.perform("messages.download", partView.part.key)
            }

            PillButton {
                visible: partView.part.state === "playable" || partView.part.state === "openable"
                text: L10n.t("Open")
                onClicked: partView.open()
            }
        }
    }
}
