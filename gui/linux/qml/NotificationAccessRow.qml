import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// Access the phone gives this computer, in a settings section: "Allowed", a spinner while a request
// waits for the phone, or a button that asks for it on the phone. `value` replaces all three when the
// phone cannot give the access ("Not Supported").
RowLayout {
    id: row

    property string title
    property bool allowed: false
    property bool pending: false
    property string value

    signal requested()

    readonly property bool asks: value.length === 0 && !allowed

    Layout.fillWidth: true
    Layout.minimumHeight: 30
    // Lined up with the switches' text and knobs.
    Layout.leftMargin: 6
    Layout.rightMargin: 6
    spacing: 10

    Text {
        Layout.fillWidth: true
        text: row.title
        color: Theme.text
        font.pixelSize: Theme.bodySize
        wrapMode: Text.Wrap
        opacity: row.enabled ? 1 : 0.4
    }

    Text {
        visible: !row.asks
        text: row.value.length > 0 ? row.value : L10n.t("Allowed")
        color: Theme.secondaryText
        font.pixelSize: Theme.bodySize
    }

    BusyIndicator {
        Layout.preferredWidth: 24
        Layout.preferredHeight: 24
        padding: 0
        visible: row.asks && row.pending
        running: visible
        Accessible.name: L10n.t("Waiting for your phone")
    }

    PillButton {
        visible: row.asks && !row.pending
        text: L10n.t("Send Request to Phone")
        onClicked: row.requested()
    }
}
