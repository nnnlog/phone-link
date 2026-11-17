import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Dialogs
import QtQuick.Layouts
import PhoneLink

// Files: the phone's storage in the file manager, whether this phone is offered when files are sent
// from this computer, and where the files the phone sends are saved (shown once this computer has
// accepted files from the phone).
SettingsPane {
    id: pane

    readonly property var files: { EngineBridge.settingsRevision; EngineBridge.transfersRevision; return JSON.parse(EngineBridge.query("files.settings", "")) }
    readonly property var phoneFiles: { EngineBridge.settingsRevision; return JSON.parse(EngineBridge.query("phoneFiles", "")) }
    readonly property bool noticeDue: phoneFiles !== null && phoneFiles.recycleNotice === true

    // After files are deleted from the phone's folder, a notice says they went to the phone's recycle
    // bin. It opens once the pane is on screen, and again after later deletions until the user chooses
    // Don't Show Again.
    function showNoticeWhenShown() {
        if (noticeDue && pane.visible && pane.Window.window !== null && pane.Window.window.visible && !recycleNotice.visible)
            recycleNotice.open()
    }

    onNoticeDueChanged: Qt.callLater(showNoticeWhenShown)
    onVisibleChanged: Qt.callLater(showNoticeWhenShown)

    Connections {
        target: pane.Window.window
        function onVisibleChanged() {
            Qt.callLater(pane.showNoticeWhenShown)
        }
    }

    SettingsSection {
        title: L10n.t("Phone Files")
        visible: pane.phoneFiles !== null && pane.phoneFiles.capable === true

        MacSwitch {
            Layout.fillWidth: true
            text: L10n.t("Show phone files in the file manager")
            checked: pane.phoneFiles !== null && pane.phoneFiles.enabled === true
            onToggled: {
                EngineBridge.perform("phoneFiles.enable", String(checked))
                checked = Qt.binding(() => pane.phoneFiles !== null && pane.phoneFiles.enabled === true)
            }
        }

        SettingsNote {
            text: L10n.f("Browse, open, copy, rename and delete your phone's files from %@", [pane.phoneFiles ? pane.phoneFiles.pathShown : ""])
        }

        RowLayout {
            Layout.fillWidth: true
            visible: pane.phoneFiles !== null && pane.phoneFiles.needsPermission === true
            spacing: 12

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2

                Text {
                    Layout.fillWidth: true
                    text: L10n.t("Your phone needs to allow access to its files")
                    color: Theme.text
                    font.pixelSize: Theme.bodySize
                    wrapMode: Text.Wrap
                }

                SettingsNote {
                    visible: pane.phoneFiles !== null && pane.phoneFiles.permissionRequested === true
                    text: L10n.t("Check your phone and allow Link to Windows to manage all files.")
                }
            }

            PillButton {
                text: pane.phoneFiles !== null && pane.phoneFiles.permissionRequested === true
                      ? L10n.t("Ask Again") : L10n.t("Send Request to Phone")
                onClicked: EngineBridge.perform("phoneFiles.requestPermission", "")
            }
        }

        PillButton {
            visible: pane.phoneFiles !== null && pane.phoneFiles.mounted === true
            text: L10n.t("Open Phone Files")
            onClicked: EngineBridge.perform("phoneFiles.open", "")
        }
    }

    SettingsSection {
        title: L10n.t("Files")

        SettingsNote {
            visible: pane.files === null
            text: L10n.t("Settings are available once Phone Link is connected.")
        }

        MacSwitch {
            Layout.fillWidth: true
            visible: pane.files !== null && pane.files.shareToggleShown === true
            text: L10n.t("Send files to this phone")
            checked: pane.files !== null && pane.files.shareEnabled === true
            onToggled: {
                EngineBridge.perform("files.shareEnabled", String(checked))
                checked = Qt.binding(() => pane.files !== null && pane.files.shareEnabled === true)
            }
        }

        SettingsNote {
            visible: pane.files !== null && pane.files.shareToggleShown === true
            text: L10n.t("Offer this phone when you send files from this computer")
        }

        RowLayout {
            Layout.fillWidth: true
            visible: pane.files !== null && pane.files.folderShown === true
            spacing: 12

            Text {
                text: L10n.t("Save received files to")
                color: Theme.text
                font.pixelSize: Theme.bodySize
            }

            Text {
                Layout.fillWidth: true
                text: pane.files && pane.files.folder ? pane.files.folder : ""
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
                horizontalAlignment: Text.AlignRight
                elide: Text.ElideMiddle
            }
        }

        RowLayout {
            Layout.alignment: Qt.AlignRight
            visible: pane.files !== null && pane.files.folderShown === true
            spacing: 8

            PillButton {
                text: L10n.t("Open Folder")
                onClicked: EngineBridge.perform("files.openFolder", "")
            }

            PillButton {
                text: L10n.t("Change...")
                onClicked: folderChooser.open()
            }
        }

        SettingsNote {
            visible: pane.files !== null && pane.files.folderShown !== true && !!pane.files.folder
            text: L10n.f("Files your phone shares with this computer are saved to %@.", [pane.files && pane.files.folder ? pane.files.folder : ""])
        }
    }

    FolderDialog {
        id: folderChooser
        title: L10n.t("Save received files to")
        onAccepted: EngineBridge.perform("files.setFolder", selectedFolder.toString())
    }

    Dialog {
        id: recycleNotice

        parent: Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(400, parent ? parent.width - 48 : 400)
        modal: true
        focus: true
        padding: 20
        closePolicy: Popup.CloseOnEscape
        // Escape reads as OK; the buttons have dismissed the notice before they close it.
        onClosed: {
            if (pane.noticeDue)
                EngineBridge.perform("phoneFiles.dismissRecycleNotice", "again")
        }

        background: Rectangle {
            radius: Theme.moduleRadius
            color: Theme.raisedBackground
            border.color: Theme.separator
        }

        contentItem: ColumnLayout {
            spacing: 10

            Text {
                Layout.fillWidth: true
                text: L10n.t("Deleted Files Go to Your Phone's Recycle Bin")
                color: Theme.text
                font.pixelSize: Theme.headlineSize - 2
                font.weight: Font.DemiBold
                wrapMode: Text.Wrap
            }

            Text {
                Layout.fillWidth: true
                text: L10n.f("Files you delete from your phone's folder are kept in “%@” on your phone for 180 days.",
                             [pane.phoneFiles ? pane.phoneFiles.recycleBin : ""])
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
                wrapMode: Text.Wrap
            }

            RowLayout {
                Layout.alignment: Qt.AlignRight
                Layout.topMargin: 8
                spacing: 8

                PillButton {
                    text: L10n.t("Don't Show Again")
                    onClicked: {
                        EngineBridge.perform("phoneFiles.dismissRecycleNotice", "never")
                        recycleNotice.close()
                    }
                }

                PillButton {
                    prominent: true
                    text: L10n.t("OK")
                    onClicked: {
                        EngineBridge.perform("phoneFiles.dismissRecycleNotice", "again")
                        recycleNotice.close()
                    }
                }
            }
        }
    }
}
