import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Templates as T
import QtQuick.Layouts
import PhoneLink

// How to let Link to Windows on the phone send notifications: only the phone can allow it, so this shows
// the link that opens the setting there, as a code to scan and in its short form. It sends nothing; the
// list column closes it once the phone reports the permission. Escape closes it too.
Dialog {
    id: help

    property string phoneName
    readonly property string shortLink: "aka.ms/notification"
    // The link the code holds, with a fresh trace id each time the help opens.
    property string link
    readonly property string code: link.length > 0 ? JSON.parse(EngineBridge.query("qr", link)) || "" : ""

    function show() {
        link = JSON.parse(EngineBridge.query("notifications.postNotificationLink", "")) || ""
        open()
    }

    parent: T.Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(380, parent ? parent.width - 48 : 380)
    modal: true
    focus: true
    padding: 24

    background: Rectangle {
        radius: Theme.moduleRadius
        color: Theme.raisedBackground
        border.color: Theme.separator
    }

    contentItem: ColumnLayout {
        spacing: 16

        Text {
            Layout.fillWidth: true
            text: L10n.t("Allow Notifications on Your Phone")
            color: Theme.text
            font.pixelSize: Theme.headlineSize + 2
            font.weight: Font.DemiBold
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
        }

        Text {
            Layout.fillWidth: true
            // A word joiner after the slash keeps the short link on one line.
            text: L10n.f("Scan this code with %@, or open %@ on it, then allow Link to Windows to send notifications.",
                         [help.phoneName, help.shortLink.replace("/", "/\u2060")])
            color: Theme.text
            font.pixelSize: Theme.bodySize
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
        }

        // White around the code in either theme, so a phone's camera reads it.
        Rectangle {
            Layout.alignment: Qt.AlignHCenter
            implicitWidth: 188
            implicitHeight: 188
            radius: 8
            color: "white"
            visible: help.code.length > 0

            Image {
                anchors.centerIn: parent
                width: 180
                height: 180
                source: help.code
                fillMode: Image.PreserveAspectFit
                smooth: false
                Accessible.role: Accessible.Graphic
                Accessible.name: L10n.f("QR code that opens %@", [help.shortLink])
            }
        }

        TextEdit {
            Layout.alignment: Qt.AlignHCenter
            text: help.shortLink
            color: Theme.text
            font.pixelSize: Theme.bodySize + 2
            font.weight: Font.DemiBold
            readOnly: true
            selectByMouse: true
            selectionColor: Theme.accent
            selectedTextColor: "white"
        }

        PillButton {
            Layout.alignment: Qt.AlignHCenter
            text: L10n.t("Maybe Later")
            onClicked: help.close()
        }
    }
}
