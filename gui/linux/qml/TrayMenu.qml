import QtQuick
import Qt.labs.platform as Platform
import PhoneLink

// The tray icon's menu: the phone's status, its controls in the order of the sidebar's, what plays on
// the phone, and the app's own commands. Its status lines and the playing title are disabled items. The
// app's own commands carry no icons. An item's second line (the ringer's next mode, the artist) is a
// disabled item below it, without an icon.
Platform.Menu {
    id: menu

    signal windowRequested()
    signal settingsRequested()
    signal aboutRequested()

    readonly property string phase: EngineBridge.phase
    readonly property bool running: phase === "running"
    readonly property bool connected: EngineBridge.phoneConnected
    readonly property bool disabled: EngineBridge.featuresDisabled
    readonly property var status: running ? JSON.parse(EngineBridge.statusJson) : ({})
    readonly property var lines: status.lines || []
    readonly property var settings: {
        EngineBridge.settingsRevision
        return running ? JSON.parse(EngineBridge.query("settings", "")) || {} : {}
    }
    readonly property var connection: {
        EngineBridge.settingsRevision
        return JSON.parse(EngineBridge.query("shell.connection", "")) || {}
    }
    readonly property var mirroring: {
        EngineBridge.mirroringRevision
        return running ? JSON.parse(EngineBridge.query("mirror.status", "")) : null
    }
    readonly property bool mirroringOffered: mirroring !== null && mirroring.offered === true
    readonly property var media: {
        EngineBridge.mediaRevision
        return running ? JSON.parse(EngineBridge.query("media", "")) || { shown: false } : { shown: false }
    }
    readonly property string phoneName: EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("Phone")

    // The groups of controls, each set off by a separator.
    readonly property bool modes: running && (!!status.dnd || !!status.ringer)
    readonly property bool reach: running && (settings.hotspotSupported === true || !!status.ring)
    readonly property bool handoff: running && (settings.canSendFiles === true || mirroringOffered)
    readonly property bool playing: running && media.shown === true

    function line(id) {
        return lines.find(item => item.id === id) || null
    }

    // While the engine does not run: what it is doing or waits for, and what the user can do.
    Platform.MenuItem {
        visible: menu.phase === "idle" || menu.phase === "starting"
        enabled: false
        text: L10n.t("Connecting...")
    }

    Platform.MenuItem {
        visible: menu.phase === "signIn" || menu.phase === "accountRemoved"
        enabled: false
        text: L10n.t("Sign In to Continue")
    }

    Platform.MenuItem {
        visible: menu.phase === "signedOut"
        enabled: false
        text: L10n.t("Signed Out")
    }

    Platform.MenuItem {
        visible: menu.phase === "signIn" || menu.phase === "accountRemoved" || menu.phase === "signedOut"
        text: L10n.t("Sign In...")
        onTriggered: {
            if (menu.phase !== "signIn")
                EngineBridge.start()
            menu.windowRequested()
        }
    }

    Platform.MenuItem {
        visible: menu.phase === "selectingPhone"
        enabled: false
        text: L10n.t("Choose a Phone")
    }

    Platform.MenuItem {
        visible: menu.phase === "selectingPhone"
        text: L10n.t("Choose Phone...")
        onTriggered: menu.windowRequested()
    }

    Platform.MenuItem {
        visible: menu.phase === "failed"
        enabled: false
        text: L10n.t("Couldn't Connect")
    }

    Platform.MenuItem {
        visible: menu.phase === "failed"
        text: L10n.t("Try Again")
        onTriggered: EngineBridge.start()
    }

    // The phone and its connection.
    Platform.MenuItem {
        visible: menu.running
        enabled: false
        text: menu.disabled ? L10n.f("%@ · Link to Windows Turned Off", [menu.phoneName])
            : menu.connected ? L10n.f("%@ · Connected", [menu.phoneName])
            : L10n.f("%@ · Not Connected", [menu.phoneName])
    }

    Platform.MenuItem {
        visible: menu.running && !menu.connected && !menu.disabled && !!menu.connection.failure
        enabled: false
        text: menu.connection.failure || ""
    }

    // Its status lines.
    Platform.MenuItem {
        readonly property var item: menu.line("battery")
        visible: item !== null
        enabled: false
        text: item ? item.text : ""
        icon.name: item ? item.icon : ""
    }

    Platform.MenuItem {
        readonly property var item: menu.line("cellular")
        visible: item !== null
        enabled: false
        text: item ? item.text : ""
        icon.name: item ? item.icon : ""
    }

    Platform.MenuItem {
        readonly property var item: menu.line("wifi")
        visible: item !== null
        enabled: false
        text: item ? item.text : ""
        icon.name: item ? item.icon : ""
    }

    Platform.MenuItem {
        readonly property var item: menu.line("bluetooth")
        visible: item !== null
        enabled: false
        text: item ? item.text : ""
        icon.name: item ? item.icon : ""
    }

    Platform.MenuItem {
        readonly property var item: menu.line("mobileData")
        visible: item !== null
        enabled: false
        text: item ? item.text : ""
        icon.name: item ? item.icon : ""
    }

    Platform.MenuItem {
        readonly property var item: menu.line("metered")
        visible: item !== null
        enabled: false
        text: item ? item.text : ""
        icon.name: item ? item.icon : ""
    }

    Platform.MenuItem {
        visible: menu.running && menu.lines.length === 0
        enabled: false
        text: L10n.t("Phone Status Unavailable")
    }

    // Do Not Disturb and the ringer.
    Platform.MenuSeparator {
        visible: menu.modes
    }

    Platform.MenuItem {
        readonly property var dnd: menu.status.dnd || null
        visible: menu.running && dnd !== null
        enabled: dnd !== null && dnd.menuEnabled === true
        checkable: dnd !== null && dnd.menuOn !== null
        checked: dnd !== null && dnd.menuOn === true
        text: dnd && dnd.menuTitle ? dnd.menuTitle : ""
        icon.name: "notifications-disabled"
        onTriggered: {
            checked = Qt.binding(() => dnd !== null && dnd.menuOn === true)
            EngineBridge.toggleDoNotDisturb()
        }
    }

    Platform.MenuItem {
        readonly property var ringer: menu.status.ringer || null
        visible: menu.running && ringer !== null
        enabled: ringer !== null && ringer.menuEnabled === true
        text: ringer && ringer.menuTitle ? ringer.menuTitle : ""
        icon.name: !ringer ? "" : ringer.mode === "silent" ? "audio-volume-muted"
            : ringer.mode === "vibrate" ? "smartphone" : "audio-volume-high"
        onTriggered: EngineBridge.cycleRinger()
    }

    Platform.MenuItem {
        visible: menu.running && !!menu.status.ringer && !!menu.status.ringer.menuHint
        enabled: false
        text: menu.status.ringer && menu.status.ringer.menuHint ? menu.status.ringer.menuHint : ""
    }

    // The hotspot and ringing the phone.
    Platform.MenuSeparator {
        visible: menu.reach
    }

    Platform.MenuItem {
        readonly property bool busy: menu.settings.hotspotBusy === true
        readonly property bool joined: menu.settings.hotspotJoined === true
        visible: menu.running && menu.settings.hotspotSupported === true
        enabled: !busy
        checkable: !busy
        checked: !busy && joined
        text: !busy ? L10n.t("Phone Hotspot")
            : joined ? L10n.t("Disconnecting from Phone Hotspot...") : L10n.t("Connecting to Phone Hotspot...")
        icon.name: "network-wireless-hotspot"
        onTriggered: {
            checked = Qt.binding(() => !busy && joined)
            EngineBridge.perform(joined ? "hotspot.disconnect" : "hotspot.connect", "")
        }
    }

    Platform.MenuItem {
        readonly property var ring: menu.status.ring || null
        visible: menu.running && ring !== null
        enabled: ring !== null && ring.menuEnabled === true
        text: ring && ring.menuTitle ? ring.menuTitle : ""
        icon.name: "preferences-desktop-notification-bell"
        onTriggered: EngineBridge.findPhone()
    }

    Platform.MenuItem {
        readonly property var ring: menu.status.ring || null
        visible: menu.running && ring !== null && !!ring.stopTitle
        text: ring && ring.stopTitle ? ring.stopTitle : ""
        icon.name: "notifications-disabled"
        onTriggered: EngineBridge.findPhone()
    }

    // Sending files and the phone's screen.
    Platform.MenuSeparator {
        visible: menu.handoff
    }

    Platform.MenuItem {
        visible: menu.running && menu.settings.canSendFiles === true
        text: L10n.t("Send Files to Phone...")
        icon.name: "document-send"
        onTriggered: EngineBridge.perform("files.showSendWindow", "")
    }

    Platform.MenuItem {
        visible: menu.running && menu.mirroringOffered
        text: L10n.t("Open Phone Screen")
        icon.name: "smartphone"
        onTriggered: EngineBridge.perform("mirror.openPhoneScreen", "")
    }

    // What plays on the phone.
    Platform.MenuSeparator {
        visible: menu.playing
    }

    Platform.MenuItem {
        visible: menu.playing
        enabled: false
        text: menu.media.title || ""
        icon.source: menu.media.art || ""
        icon.name: menu.media.art ? "" : "audio-x-generic"
    }

    Platform.MenuItem {
        readonly property string subtitle: [menu.media.artist, menu.media.app].filter(part => !!part).join(" · ")
        visible: menu.playing && subtitle.length > 0
        enabled: false
        text: subtitle
    }

    Platform.MenuItem {
        visible: menu.playing
        text: menu.media.playing ? L10n.t("Pause") : L10n.t("Play")
        icon.name: menu.media.playing ? "media-playback-pause" : "media-playback-start"
        onTriggered: EngineBridge.perform("media.playPause", "")
    }

    Platform.MenuItem {
        visible: menu.playing
        text: L10n.t("Next Track")
        icon.name: "media-skip-forward"
        onTriggered: EngineBridge.perform("media.next", "")
    }

    Platform.MenuItem {
        visible: menu.playing
        text: L10n.t("Previous Track")
        icon.name: "media-skip-backward"
        onTriggered: EngineBridge.perform("media.previous", "")
    }

    // The app's own commands.
    Platform.MenuSeparator {}

    Platform.MenuItem {
        text: L10n.t("Open Phone Link")
        onTriggered: menu.windowRequested()
    }

    Platform.MenuItem {
        text: L10n.t("Settings...")
        onTriggered: menu.settingsRequested()
    }

    Platform.MenuSeparator {}

    Platform.MenuItem {
        text: L10n.t("About Phone Link")
        onTriggered: menu.aboutRequested()
    }

    Platform.MenuItem {
        text: L10n.t("Quit Phone Link")
        onTriggered: EngineBridge.perform("app.quit", "")
    }
}
