import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Templates as T
import QtQuick.Layouts
import Qt.labs.settings
import PhoneLink

// The explanation shown the first time call control is switched on on this computer: calls placed or
// answered here use its microphone and speakers. *Turn On* switches it on; Return does too, Escape
// cancels.
Dialog {
    id: dialog

    // Whether switching call control on still explains this first.
    function needed() {
        return String(prefs.value("controlIntroShown", false)) !== "true"
    }

    function turnOn() {
        prefs.setValue("controlIntroShown", true)
        EngineBridge.perform("calls.enable", "true")
        dialog.accept()
    }

    parent: T.Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(480, parent ? parent.width - 48 : 480)
    modal: true
    focus: true
    padding: 24

    // Shared by every copy of this dialog through the settings file, so one acknowledgement counts for all.
    Settings {
        id: prefs
        category: "Calls"
    }

    background: Rectangle {
        radius: Theme.moduleRadius
        color: Theme.raisedBackground
        border.color: Theme.separator
    }

    contentItem: ColumnLayout {
        spacing: 16
        focus: true
        Keys.onReturnPressed: dialog.turnOn()
        Keys.onEnterPressed: dialog.turnOn()

        Icon {
            Layout.alignment: Qt.AlignHCenter
            name: "phoneConnection"
            size: 40
            color: Theme.accent
        }

        Text {
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            text: L10n.t("Calls on This Computer")
            color: Theme.text
            font.pixelSize: Theme.titleSize - 2
            font.weight: Font.DemiBold
            wrapMode: Text.Wrap
        }

        Text {
            Layout.fillWidth: true
            text: L10n.t("This computer can place, answer, decline, and end your phone's calls over Bluetooth.")
            color: Theme.text
            font.pixelSize: Theme.bodySize
            wrapMode: Text.Wrap
        }

        Text {
            Layout.fillWidth: true
            text: L10n.t("Calls you place or answer here use this computer's microphone and speakers.")
            color: Theme.text
            font.pixelSize: Theme.bodySize
            font.weight: Font.Medium
            wrapMode: Text.Wrap
        }

        RowLayout {
            Layout.fillWidth: true
            Layout.topMargin: 4

            PillButton {
                text: L10n.t("Cancel")
                onClicked: dialog.reject()
            }

            Item {
                Layout.fillWidth: true
            }

            PillButton {
                prominent: true
                text: L10n.t("Turn On")
                onClicked: dialog.turnOn()
            }
        }
    }
}
