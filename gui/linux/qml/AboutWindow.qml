import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import QtQuick.Window
import PhoneLink

// About Phone Link: the app's icon, name and version; links to its repository (when the build names
// one): the project, its releases, reporting an issue and the license; and the notice that the app is
// not Microsoft's.
Window {
    id: window

    readonly property var about: JSON.parse(EngineBridge.query("shell.about", "")) || {}
    // The repository's home page, for a GitHub repository.
    readonly property string home: {
        const repository = (about.repository || "").replace(/\/+$/, "")
        return /^https:\/\/github\.com\/[^\/]+\/[^\/]+$/.test(repository) ? repository : ""
    }
    readonly property var links: home.length === 0 ? [] : [
        { title: "GitHub", url: home },
        { title: L10n.t("Releases"), url: home + "/releases" },
        { title: L10n.t("Report an Issue"), url: home + "/issues" },
        { title: L10n.t("License"), url: home + "/blob/HEAD/LICENSE" }
    ]

    readonly property int fittedHeight: Math.ceil(column.implicitHeight) + 48

    title: L10n.t("About Phone Link")
    width: 340
    height: fittedHeight
    minimumWidth: 340
    maximumWidth: 340
    minimumHeight: fittedHeight
    maximumHeight: fittedHeight
    color: Theme.windowBackground

    palette.window: Theme.windowBackground
    palette.windowText: Theme.text
    palette.base: Theme.fieldBackground
    palette.text: Theme.text
    palette.button: Theme.controlBackground
    palette.buttonText: Theme.text
    palette.highlight: Theme.accent
    palette.highlightedText: "white"
    palette.placeholderText: Theme.secondaryText
    palette.light: Theme.paletteLight
    palette.midlight: Theme.paletteMidlight
    palette.mid: Theme.paletteMid
    palette.dark: Theme.paletteDark
    palette.shadow: "black"
    palette.toolTipBase: Theme.toolTipBackground
    palette.toolTipText: Theme.text

    Shortcut {
        sequence: "Escape"
        onActivated: window.close()
    }

    ColumnLayout {
        id: column
        x: 24
        y: 24
        width: window.width - 48
        spacing: 8

        Image {
            Layout.alignment: Qt.AlignHCenter
            Layout.preferredWidth: 64
            Layout.preferredHeight: 64
            visible: !!window.about.icon
            source: window.about.icon || ""
            sourceSize.width: 128
            sourceSize.height: 128
        }

        Text {
            Layout.fillWidth: true
            Layout.topMargin: 4
            text: "Phone Link"
            color: Theme.text
            font.pixelSize: Theme.headlineSize
            font.weight: Font.Bold
            horizontalAlignment: Text.AlignHCenter
        }

        TextEdit {
            Layout.fillWidth: true
            text: L10n.t("Version") + " " + (window.about.version || "")
            color: Theme.secondaryText
            font.pixelSize: Theme.subheadSize
            horizontalAlignment: TextEdit.AlignHCenter
            readOnly: true
            selectByMouse: true
            selectionColor: Theme.accent
            selectedTextColor: "white"
        }

        Flow {
            Layout.alignment: Qt.AlignHCenter
            Layout.maximumWidth: column.width
            Layout.topMargin: 8
            visible: window.links.length > 0
            spacing: 4

            Repeater {
                model: window.links

                Row {
                    required property var modelData
                    required property int index
                    spacing: 4

                    Text {
                        visible: parent.index > 0
                        text: "·"
                        color: Theme.secondaryText
                        font.pixelSize: Theme.captionSize
                    }

                    AbstractButton {
                        id: link
                        hoverEnabled: true
                        Accessible.role: Accessible.Link
                        Accessible.name: parent.modelData.title
                        onClicked: Qt.openUrlExternally(parent.modelData.url)

                        contentItem: Text {
                            text: link.parent.modelData.title
                            color: Theme.accent
                            font.pixelSize: Theme.captionSize
                            font.underline: link.hovered || link.visualFocus
                        }

                        background: Item {}
                    }
                }
            }
        }

        Text {
            Layout.fillWidth: true
            Layout.topMargin: 8
            text: L10n.t("Phone Link for Linux is an independent project and is neither affiliated with, nor authorized, sponsored, or approved by, Microsoft Corporation.")
            color: Theme.secondaryText
            font.pixelSize: Theme.captionSize
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
        }
    }
}
