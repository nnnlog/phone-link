import QtCore
import QtQuick
import QtQuick.Dialogs as Dialogs
import QtQuick.Layouts
import PhoneLink

// Settings › Advanced › Log: the size of the app's log, a copy saved where the user chooses (then
// shown in the file manager), and clearing it. The log is there from launch, signed in or not.
SettingsSection {
    id: section

    // On screen: in the current tab of an open Settings window.
    readonly property bool shown: visible && Window.window !== null && Window.window.visible
    readonly property var log: {
        EngineBridge.settingsRevision
        // Read again whenever the pane shows: the log grows meanwhile.
        section.shown
        return JSON.parse(EngineBridge.query("logs", "")) || {}
    }
    readonly property double size: log.size || 0

    // "12.4 MB": decimal units, as the file manager shows sizes.
    function sizeText(bytes) {
        if (bytes < 1000)
            return bytes === 1 ? L10n.f("%@ byte", [bytes]) : L10n.f("%@ bytes", [bytes])
        let value = bytes / 1000
        let unit = 0
        // A number from 100 up shows whole, so from 999.5 on it would read 1000: the next unit takes it.
        while (value >= 999.5 && unit < 2) {
            value /= 1000
            unit++
        }
        const number = Number(value).toLocaleString(Qt.locale(), "f", value >= 100 ? 0 : 1)
        switch (unit) {
        case 0: return L10n.f("%@ KB", [number])
        case 1: return L10n.f("%@ MB", [number])
        default: return L10n.f("%@ GB", [number])
        }
    }

    title: L10n.t("Log")

    RowLayout {
        Layout.fillWidth: true
        spacing: 12

        Text {
            Layout.fillWidth: true
            text: L10n.t("Log Size")
            color: Theme.text
            font.pixelSize: Theme.bodySize
        }

        Text {
            text: section.sizeText(section.size)
            color: Theme.secondaryText
            font.pixelSize: Theme.bodySize
        }
    }

    RowLayout {
        spacing: 8

        PillButton {
            enabled: section.log.installed === true
            text: L10n.t("Save Log...")
            onClicked: {
                saveDialog.selectedFile = saveDialog.currentFolder + "/"
                    + L10n.f("Phone Link Log %@.log", [Qt.formatDateTime(new Date(), "yyyy-MM-dd HH.mm")])
                saveDialog.open()
            }
        }

        PillButton {
            enabled: section.size > 0
            destructive: true
            text: L10n.t("Clear Log...")
            onClicked: confirm.open()
        }
    }

    SettingsNote {
        text: section.log.installed === true
              ? L10n.t("Phone Link records what it does on this computer to help diagnose problems. Account and device IDs, names, numbers, and other personal details are replaced with REDACTED tags as they are recorded. The log stays on this computer until you save a copy.")
              : L10n.t("Phone Link couldn't open its log file, so it writes its log to standard error.")
    }

    Dialogs.FileDialog {
        id: saveDialog
        title: L10n.t("Save Log...")
        fileMode: Dialogs.FileDialog.SaveFile
        currentFolder: StandardPaths.writableLocation(StandardPaths.DocumentsLocation)
        onAccepted: EngineBridge.perform("logs.save", selectedFile.toString())
    }

    MessageDialog {
        id: confirm
        heading: L10n.t("Clear the Log?")
        message: L10n.t("Everything Phone Link has recorded so far will be deleted.")
        acceptText: L10n.t("Clear")
        destructive: true
        onAccepted: EngineBridge.perform("logs.clear", "")
    }
}
