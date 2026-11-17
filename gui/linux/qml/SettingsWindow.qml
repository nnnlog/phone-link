import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// The Settings window: one tab per part of the app, each a SettingsPane in its own file.
ApplicationWindow {
    id: window

    readonly property var tabs: [
        { key: "general", title: L10n.t("General"), icon: "gear", source: "SettingsGeneralPane.qml" },
        { key: "notifications", title: L10n.t("Notifications"), icon: "bell", source: "SettingsNotificationsPane.qml" },
        { key: "messages", title: L10n.t("Messages"), icon: "message", source: "SettingsMessagesPane.qml" },
        { key: "calls", title: L10n.t("Calls"), icon: "phone", source: "SettingsCallsPane.qml" },
        { key: "photos", title: L10n.t("Photos"), icon: "photo", source: "SettingsPhotosPane.qml" },
        { key: "files", title: L10n.t("Files"), icon: "upload", source: "SettingsFilesPane.qml" },
        { key: "camera", title: L10n.t("Camera"), icon: "device", source: "SettingsCameraPane.qml" },
        { key: "phoneScreen", title: L10n.t("Phone Screen"), icon: "handset", source: "SettingsPhoneScreenPane.qml" },
        { key: "account", title: L10n.t("Account"), icon: "person", source: "SettingsAccountPane.qml" },
        { key: "advanced", title: L10n.t("Advanced"), icon: "sliders", source: "SettingsAdvancedPane.qml" }
    ]
    property int current: 0
    // The desktop has a system tray (Main.qml's tray icon knows); the General pane reads it.
    property bool trayAvailable: false
    // The pane shown last is shown again once every pane has loaded; from then on it is remembered.
    property bool restored: false

    // Shows the tab `key` ("general", "notifications", …).
    function showPane(key) {
        const index = tabs.findIndex(tab => tab.key === key)
        if (index >= 0)
            current = index
        show()
        raise()
        requestActivate()
    }

    function restorePane() {
        const index = tabs.findIndex(tab => tab.key === ShellPreferences.settingsPane)
        if (index >= 0)
            current = index
        restored = true
    }

    Component.onCompleted: Qt.callLater(restorePane)
    onCurrentChanged: {
        if (restored)
            ShellPreferences.settingsPane = tabs[current].key
    }

    title: L10n.t("Settings")
    width: 760
    height: 560
    minimumWidth: 660
    minimumHeight: 440
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

    header: Rectangle {
        implicitHeight: tabRow.implicitHeight + 16
        color: Theme.windowBackground

        Flickable {
            anchors.fill: parent
            anchors.margins: 8
            contentWidth: tabRow.implicitWidth
            boundsBehavior: Flickable.StopAtBounds
            clip: true

            Row {
                id: tabRow
                x: Math.max(0, (parent.width - implicitWidth) / 2)
                spacing: 2

                Repeater {
                    model: window.tabs

                    delegate: Rectangle {
                        id: tab

                        required property var modelData
                        required property int index
                        readonly property bool selected: window.current === index

                        width: Math.max(64, label.implicitWidth + 16)
                        height: 52
                        radius: 8
                        color: selected ? Theme.selection : hover.hovered ? Theme.hover : "transparent"

                        Accessible.role: Accessible.PageTab
                        Accessible.name: label.text
                        Accessible.onPressAction: window.current = index

                        Icon {
                            anchors.horizontalCenter: parent.horizontalCenter
                            y: 7
                            name: tab.modelData.icon
                            size: 20
                            color: tab.selected ? Theme.accent : Theme.secondaryText
                        }

                        Text {
                            id: label
                            anchors.horizontalCenter: parent.horizontalCenter
                            y: 31
                            text: tab.modelData.title
                            color: tab.selected ? Theme.accent : Theme.text
                            font.pixelSize: Theme.captionSize
                        }

                        HoverHandler {
                            id: hover
                        }

                        TapHandler {
                            onTapped: window.current = tab.index
                        }
                    }
                }
            }
        }

        Rectangle {
            anchors.bottom: parent.bottom
            width: parent.width
            height: 1
            color: Theme.separator
        }
    }

    StackLayout {
        anchors.fill: parent
        currentIndex: window.current

        Repeater {
            model: window.tabs

            delegate: Loader {
                id: loader

                required property var modelData

                source: modelData.source

                Connections {
                    target: loader.item
                    ignoreUnknownSignals: true

                    function onSignedOut() {
                        window.close()
                    }
                }
            }
        }
    }
}
