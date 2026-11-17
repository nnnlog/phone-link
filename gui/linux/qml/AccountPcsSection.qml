import QtQuick
import QtQuick.Layouts
import PhoneLink

// The PCs of the Microsoft account. The phone finds this computer's enrollment by name: an older
// enrollment of this computer left on the account can be picked instead, and the phone then shows this
// computer as delayed and without its controls. Unlinking those older PCs here removes them.
SettingsSection {
    id: section

    readonly property var pcs: {
        EngineBridge.settingsRevision
        EngineBridge.phonesJson
        return JSON.parse(EngineBridge.query("account.pcs", "")) || []
    }
    property var confirming: null

    function stamp(ms) {
        return ms ? new Date(ms).toLocaleDateString(Qt.locale(), Locale.ShortFormat) + " " + Dates.time(ms)
                  : L10n.t("unknown")
    }

    function detail(pc) {
        return L10n.f("%@ · linked %@ · last seen %@", [pc.app, stamp(pc.registeredMs), stamp(pc.lastSeenMs)])
    }

    title: L10n.t("PCs")

    Text {
        Layout.fillWidth: true
        visible: section.pcs.length === 0
        text: L10n.t("No PCs to show.")
        color: Theme.secondaryText
        font.pixelSize: Theme.bodySize
    }

    Repeater {
        model: section.pcs

        delegate: RowLayout {
            id: row

            required property var modelData

            Layout.fillWidth: true
            spacing: 12

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2

                Text {
                    id: pcName
                    Layout.fillWidth: true
                    text: row.modelData.name.length > 0 ? row.modelData.name : L10n.t("Unnamed PC")
                    color: Theme.text
                    font.pixelSize: Theme.bodySize
                    elide: Text.ElideRight
                }

                Text {
                    Layout.fillWidth: true
                    text: section.detail(row.modelData)
                    color: Theme.secondaryText
                    font.pixelSize: Theme.captionSize
                    wrapMode: Text.Wrap
                }
            }

            Text {
                visible: row.modelData.thisComputer === true
                text: L10n.t("This Computer")
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
            }

            PillButton {
                visible: row.modelData.thisComputer !== true
                enabled: row.modelData.unlinking !== true
                text: L10n.t("Unlink...")
                // The PC it unlinks, as every row's button says the same.
                Accessible.description: pcName.text
                onClicked: {
                    section.confirming = row.modelData
                    confirm.open()
                }
            }
        }
    }

    SettingsNote {
        text: L10n.t("Unlink PCs you no longer use. An older entry for this computer can keep your phone from reaching it.")
    }

    MessageDialog {
        id: confirm
        heading: section.confirming && section.confirming.name.length > 0
                 ? L10n.f("Unlink %@?", [section.confirming.name]) : L10n.t("Unlink This PC?")
        message: section.confirming
                 ? L10n.f("%@. The PC will be removed from your Microsoft account and can't reach your phone until it's linked again.", [section.detail(section.confirming)])
                 : ""
        acceptText: L10n.t("Unlink")
        destructive: true
        onAccepted: EngineBridge.perform("account.unlinkPc", section.confirming.id)
        onClosed: section.confirming = null
    }
}
