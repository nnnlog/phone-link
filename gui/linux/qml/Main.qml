import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import Qt.labs.platform as Platform
import PhoneLink

ApplicationWindow {
    id: root

    readonly property bool running: EngineBridge.phase === "running"
    // Mirroring reads the primary screen at launch, and again once this window is framed (its first
    // activation), when a window's title bar can be read; desktop.h follows the screen's own changes.
    property bool displayFramed: false
    readonly property bool messagesShown: running && visible && visibility !== Window.Minimized
        && content.item !== null && content.item.section === "messages"
    // Each part's windows and app-wide handlers (the <Part>Host.qml files).
    readonly property var hosts: [messagesHost, notificationsHost, callsHost, photosHost, filesHost, mirrorHost, shellHost]
    // The tray icon shows: the desktop has a tray and the user keeps the icon. Without it a launch in the
    // background would leave nothing on screen, so the window opens at every launch.
    readonly property bool trayShown: tray.available && ShellPreferences.showTrayIcon

    function present() {
        root.show()
        root.raise()
        EngineBridge.perform("app.activationToken", "")
        root.requestActivate()
    }

    function showSettings() {
        settingsWindow.show()
        settingsWindow.raise()
        settingsWindow.requestActivate()
    }

    // Quitting ends the phone session first (Rust bounds the wait), then the shell's QML quits.
    function quit() {
        EngineBridge.perform("app.quit", "")
    }

    function reportForeground() {
        EngineBridge.perform("app.foreground", String(Qt.application.state === Qt.ApplicationActive))
    }

    function reportWindowFocus() {
        EngineBridge.perform("app.windowFocused", String(active))
    }

    function reportMessagesView() {
        EngineBridge.perform("messages.viewActive", String(messagesShown))
    }

    title: "Phone Link"
    width: 1100
    height: 720
    minimumWidth: 1000
    minimumHeight: 560
    // Shown in onCompleted unless a tray icon can reopen it and it was started at login (in the
    // background) or the user turned off opening it at launch.
    visible: false
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

    Component.onCompleted: {
        const background = Qt.application.arguments.includes("--background") || !ShellPreferences.showWindowAtLaunch
        if (!background || !trayShown)
            root.show()
        EngineBridge.perform("mirror.display", "")
        EngineBridge.start()
        reportForeground()
        reportWindowFocus()
        reportMessagesView()
    }
    onActiveChanged: reportWindowFocus()
    onMessagesShownChanged: reportMessagesView()
    // Without a tray there is no way back to a hidden window, so closing it quits. With the tray icon
    // hidden by the user, Phone Link keeps running and opening it again shows the window.
    onClosing: close => {
        close.accepted = false
        root.hide()
        if (!tray.available)
            root.quit()
    }

    Shortcut {
        sequence: "Ctrl+Q"
        context: Qt.ApplicationShortcut
        onActivated: root.quit()
    }

    Shortcut {
        sequence: "Ctrl+Shift+R"
        context: Qt.ApplicationShortcut
        onActivated: EngineBridge.perform("app.refresh", "")
    }

    Shortcut {
        sequence: "Ctrl+0"
        context: Qt.ApplicationShortcut
        onActivated: root.present()
    }

    // One Ctrl+W for every window: a window's own shortcut would also match while its child window
    // (Settings) is focused, and two matches cancel each other. A window counts as active while its
    // child window is focused, so the children are asked first.
    Shortcut {
        sequence: "Ctrl+W"
        context: Qt.ApplicationShortcut
        onActivated: {
            const windows = [settingsWindow]
            root.hosts.forEach(host => windows.push(...host.windows()))
            windows.push(root)
            const focused = windows.find(window => window && window.active)
            if (focused)
                focused.close()
        }
    }

    Shortcut {
        sequence: "Ctrl+,"
        context: Qt.ApplicationShortcut
        enabled: root.running
        onActivated: root.showSettings()
    }

    Platform.SystemTrayIcon {
        id: tray
        visible: ShellPreferences.showTrayIcon
        icon.name: "phonelink"
        tooltip: "Phone Link"
        // Without a tray, the menu's fallback (KDE's platform theme) would keep Escape from every window.
        menu: available ? trayMenu : null
        onActivated: reason => {
            if (reason !== Platform.SystemTrayIcon.Context)
                root.present()
        }
    }

    TrayMenu {
        id: trayMenu
        onWindowRequested: root.present()
        onSettingsRequested: root.showSettings()
        onAboutRequested: shellHost.showAbout()
    }

    Connections {
        target: Qt.application
        function onStateChanged() {
            root.reportForeground()
        }
    }

    Connections {
        target: root
        enabled: !root.displayFramed
        function onActiveChanged() {
            if (!root.active)
                return
            root.displayFramed = true
            EngineBridge.perform("mirror.display", "")
        }
    }

    Connections {
        target: EngineBridge
        function onPhaseChanged() {
            if (EngineBridge.phase === "running") {
                Qt.callLater(root.reportWindowFocus)
                Qt.callLater(root.reportMessagesView)
            }
            // Signing in and choosing the phone happen in the window, whether or not it was open.
            if (EngineBridge.phase === "signIn" || EngineBridge.phase === "selectingPhone")
                root.present()
        }
        function onAreaEvent(area, name, payload) {
            if (area !== "shell")
                return
            switch (name) {
            case "quit":
                Qt.quit()
                break
            case "showSettings":
                settingsWindow.showPane(JSON.parse(payload).pane)
                break
            case "promptWithdrawn":
                statusPrompt.withdraw(JSON.parse(payload).kind)
                break
            }
        }
        function onShowSection(section) {
            root.present()
            if (section.length > 0 && content.item && content.item.showSection)
                content.item.showSection(section)
        }
        function onShowNotification(key) {
            root.present()
            if (content.item && content.item.showNotification)
                content.item.showNotification(key)
        }
        function onFailure(title, message) {
            toast.show(title, message)
        }
        function onStatusPrompt(kind) {
            statusPrompt.ask(kind)
        }
    }

    MessagesHost {
        id: messagesHost
    }

    NotificationsHost {
        id: notificationsHost
    }

    CallsHost {
        id: callsHost
    }

    PhotosHost {
        id: photosHost
    }

    FilesHost {
        id: filesHost
    }

    MirrorHost {
        id: mirrorHost
    }

    ShellHost {
        id: shellHost
    }

    Loader {
        id: content
        anchors.fill: parent
        sourceComponent: EngineBridge.phase === "running" ? mainView : stateView
    }

    Component {
        id: mainView
        MainView {
            onSettingsRequested: root.showSettings()
        }
    }

    Component {
        id: stateView
        EngineStateView {}
    }

    SettingsWindow {
        id: settingsWindow
        visible: false
        trayAvailable: tray.available
    }

    // A failed action: shown for a few seconds (longer while the pointer is on it), or until dismissed.
    Rectangle {
        id: toast

        property var queue: []
        readonly property var current: queue.length > 0 ? queue[0] : null

        function show(title, message) {
            queue = queue.concat([{ title: title, message: message }])
        }

        function dismiss() {
            queue = queue.slice(1)
        }

        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: 20
        width: Math.min(440, parent.width - 40)
        implicitHeight: toastRow.implicitHeight + 24
        z: 20
        visible: current !== null
        radius: Theme.moduleRadius
        color: Theme.raisedBackground
        border.color: Theme.separator
        Accessible.role: Accessible.AlertMessage
        Accessible.name: current ? current.title + ". " + current.message : ""
        onCurrentChanged: {
            if (current !== null)
                toastTimer.restart()
        }

        Timer {
            id: toastTimer
            interval: 6000
            running: toast.current !== null && !toastHover.hovered
            onTriggered: toast.dismiss()
        }

        HoverHandler {
            id: toastHover
        }

        TapHandler {
            onTapped: toast.dismiss()
        }

        RowLayout {
            id: toastRow
            anchors.fill: parent
            anchors.margins: 12
            spacing: 10

            Icon {
                Layout.alignment: Qt.AlignTop
                Layout.topMargin: 2
                name: "warning"
                size: 18
                color: Theme.warning
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2

                Text {
                    Layout.fillWidth: true
                    text: toast.current ? toast.current.title : ""
                    color: Theme.text
                    font.pixelSize: Theme.bodySize
                    font.weight: Font.DemiBold
                    wrapMode: Text.Wrap
                }

                Text {
                    Layout.fillWidth: true
                    visible: text.length > 0
                    text: toast.current ? toast.current.message : ""
                    color: Theme.secondaryText
                    font.pixelSize: Theme.subheadSize
                    wrapMode: Text.Wrap
                }
            }

            IconButton {
                Layout.alignment: Qt.AlignTop
                implicitWidth: 22
                implicitHeight: 22
                iconName: "close"
                label: L10n.t("Dismiss")
                onClicked: toast.dismiss()
            }
        }
    }

    // A prompt from a control: what it needs on the phone first, or a confirmation; the accept button
    // runs the prompt's action.
    MessageDialog {
        id: statusPrompt

        property var queue: []
        readonly property string kind: queue.length > 0 ? queue[0] : ""
        readonly property var texts: {
            switch (kind) {
            case "dndNotificationsPermission":
                return [L10n.t("Do Not Disturb Needs Notification Access"),
                        L10n.t("To turn Do Not Disturb on or off from this computer, allow notification access for Link to Windows on your phone."),
                        L10n.t("Allow on Phone")]
            case "ringerNotificationsPermission":
                return [L10n.t("Ringer Control Needs Notification Access"),
                        L10n.t("To change the ringer from this computer, allow notification access for Link to Windows on your phone."),
                        L10n.t("Allow on Phone")]
            case "dndRuleDisabled":
                return [L10n.t("Allow Do Not Disturb Changes on Your Phone"),
                        L10n.t("Your phone doesn't let Link to Windows change Do Not Disturb. Open the setting on your phone to allow it."),
                        L10n.t("Open Settings on Phone")]
            case "dndTurnOffFailed":
                return [L10n.t("Couldn't Turn Off Do Not Disturb"),
                        L10n.t("Your phone didn't confirm the change. Open its Do Not Disturb settings to turn it off there."),
                        L10n.t("Open Settings on Phone")]
            case "stopRinging":
                return [L10n.t("Stop Ringing Your Phone?"), L10n.t("Your phone will stop ringing."), L10n.t("Stop Ringing")]
            default:
                return ["", "", ""]
            }
        }

        function ask(newKind) {
            if (!queue.includes(newKind))
                queue = queue.concat([newKind])
            if (!visible) {
                if (!root.visible)
                    root.present()
                open()
            }
        }

        // A prompt that no longer applies: the one on screen stays, a queued one is not shown.
        function withdraw(oldKind) {
            queue = queue.filter((queued, index) => index === 0 && visible || queued !== oldKind)
        }

        heading: texts[0]
        message: texts[1]
        acceptText: texts[2]
        onAccepted: EngineBridge.perform("status.acceptPrompt", kind)
        onClosed: {
            queue = queue.slice(1)
            if (queue.length > 0)
                Qt.callLater(open)
        }
    }
}
