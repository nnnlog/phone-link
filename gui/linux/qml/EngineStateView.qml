import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// What the window shows while the engine is not running: a spinner while it starts, the sign-in page,
// the phone picker (and pairing), and what went wrong.
Rectangle {
    id: view

    readonly property string phase: EngineBridge.phase

    color: Theme.windowBackground

    Loader {
        anchors.fill: parent
        active: view.phase === "signIn"
        sourceComponent: SignInView {}
    }

    // The column stays centered while it fits; a short window scrolls it (the picker's QR code).
    Flickable {
        id: scroller
        anchors.fill: parent
        visible: view.phase !== "signIn"
        contentWidth: width
        contentHeight: Math.max(height, column.implicitHeight + 80)
        boundsBehavior: Flickable.StopAtBounds
        clip: true
        ScrollBar.vertical: ScrollBar {
            policy: scroller.contentHeight > scroller.height ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff
        }

        ColumnLayout {
            id: column
            x: (scroller.width - width) / 2
            y: (scroller.contentHeight - implicitHeight) / 2
            width: Math.min(420, scroller.width - 80)
            spacing: 14

            BusyIndicator {
                Layout.alignment: Qt.AlignHCenter
                visible: view.phase === "starting" || view.phase === "idle"
                running: visible
            }

            EmptyState {
                Layout.fillWidth: true
                iconName: {
                    switch (view.phase) {
                    case "selectingPhone": return "device"
                    case "failed": return "warning"
                    case "accountRemoved":
                    case "signedOut": return "person"
                    default: return ""
                    }
                }
                title: {
                    switch (view.phase) {
                    case "selectingPhone": return L10n.t("Choose Your Phone")
                    case "failed": return L10n.t("Phone Link Couldn't Connect")
                    case "accountRemoved": return L10n.t("Sign In Again")
                    case "signedOut": return L10n.t("Signed Out")
                    default: return L10n.t("Connecting to your phone...")
                    }
                }
                description: {
                    switch (view.phase) {
                    case "failed": return EngineBridge.message
                    case "accountRemoved": return L10n.t("This computer was removed from your Microsoft account's devices.")
                    case "signedOut": return L10n.t("Sign in with the Microsoft account your phone uses with Link to Windows.")
                    default: return ""
                    }
                }
            }

            // The engine's own words, for a bug report; selectable so they can be copied.
            TextEdit {
                Layout.fillWidth: true
                visible: view.phase === "failed" && text.length > 0
                text: EngineBridge.messageDetail
                color: Theme.secondaryText
                font.pixelSize: Theme.captionSize
                horizontalAlignment: TextEdit.AlignHCenter
                wrapMode: TextEdit.Wrap
                readOnly: true
                selectByMouse: true
                selectionColor: Theme.accent
                selectedTextColor: "white"
            }

            Loader {
                Layout.fillWidth: true
                active: view.phase === "selectingPhone"
                visible: active
                sourceComponent: PhonePickerView {}
            }

            PillButton {
                Layout.alignment: Qt.AlignHCenter
                visible: view.phase === "failed" || view.phase === "accountRemoved" || view.phase === "signedOut"
                prominent: true
                text: view.phase === "failed" ? L10n.t("Try Again") : L10n.t("Sign In")
                onClicked: EngineBridge.start()
            }
        }
    }
}
