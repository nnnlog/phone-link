import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Templates as T
import QtQuick.Layouts
import PhoneLink

// The shell's windows and app-wide handlers: the window that pairs a new phone, About Phone Link, and
// the alert of a hotspot join that did not finish, which offers the network's password. Main.qml keeps
// one for the app's lifetime.
Item {
    id: host

    // This part's open windows, for Main.qml's Ctrl+W.
    function windows() {
        return [pairingWindow.item, aboutWindow.item].filter(window => window)
    }

    function present(window) {
        window.show()
        window.raise()
        window.requestActivate()
    }

    function showPairing() {
        pairingWindow.active = true
        present(pairingWindow.item)
    }

    function showAbout() {
        aboutWindow.active = true
        present(aboutWindow.item)
    }

    Connections {
        target: EngineBridge

        function onAreaEvent(area, name, payload) {
            if (area !== "shell")
                return
            switch (name) {
            case "showPairing":
                host.showPairing()
                break
            case "paired":
                if (pairingWindow.item)
                    pairingWindow.item.close()
                break
            case "hotspotJoin":
                hotspotJoin.ask(JSON.parse(payload))
                break
            }
        }
    }

    // Created when asked for and destroyed once closed: its view starts a session when it appears.
    Loader {
        id: pairingWindow
        active: false

        sourceComponent: PairingWindow {
            onVisibleChanged: {
                if (!visible)
                    Qt.callLater(() => pairingWindow.active = false)
            }
        }
    }

    Loader {
        id: aboutWindow
        active: false

        sourceComponent: AboutWindow {
            onVisibleChanged: {
                if (!visible)
                    Qt.callLater(() => aboutWindow.active = false)
            }
        }
    }

    // Holds the text the hotspot alert copies to the clipboard.
    TextEdit {
        id: clipboard
        visible: false
    }

    // A hotspot join that did not finish: the network can be joined by hand with its password.
    Dialog {
        id: hotspotJoin

        property var outcome: ({})

        function ask(newOutcome) {
            outcome = newOutcome
            EngineBridge.showSection("")
            open()
        }

        parent: T.Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(400, parent ? parent.width - 48 : 400)
        modal: true
        focus: true
        padding: 20

        background: Rectangle {
            radius: Theme.moduleRadius
            color: Theme.raisedBackground
            border.color: Theme.separator
        }

        contentItem: ColumnLayout {
            spacing: 10

            Text {
                Layout.fillWidth: true
                text: hotspotJoin.outcome.title || ""
                color: Theme.text
                font.pixelSize: Theme.headlineSize - 2
                font.weight: Font.DemiBold
                wrapMode: Text.Wrap
            }

            Text {
                Layout.fillWidth: true
                visible: text.length > 0
                text: hotspotJoin.outcome.message || ""
                color: Theme.secondaryText
                font.pixelSize: Theme.bodySize
                wrapMode: Text.Wrap
            }

            RowLayout {
                Layout.alignment: Qt.AlignRight
                Layout.topMargin: 8
                spacing: 8

                PillButton {
                    text: L10n.t("Copy Password")
                    onClicked: {
                        clipboard.text = hotspotJoin.outcome.passphrase || ""
                        clipboard.selectAll()
                        clipboard.copy()
                        clipboard.text = ""
                        hotspotJoin.close()
                    }
                }

                PillButton {
                    prominent: true
                    text: L10n.t("OK")
                    onClicked: hotspotJoin.close()
                }
            }
        }
    }
}
