import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// The Apps section: the phone's recent apps, favorites and every app as grids, and — once the phone
// lists five apps — a search over them. A click opens the app from the phone in a window on this
// computer; a recent task's document opens on this computer or in the browser instead when the phone
// hands documents over. Apps are dragged onto the favorites to add them and back onto every app to
// remove them; their menus do the same.
Rectangle {
    id: column

    property var view: null
    // The phone's name; a phone without one is "your phone", and "Your phone" where it starts a sentence.
    readonly property string phoneName: EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("your phone")
    readonly property string phoneNameAtStart: EngineBridge.phoneName.length > 0 ? EngineBridge.phoneName : L10n.t("Your phone")
    readonly property string query: search.text.trim()
    readonly property bool searchOffered: view !== null && view.searchOffered === true
    readonly property bool searching: searchOffered && query.length > 0
    readonly property string mode: view === null || view.visible !== true ? "unavailable"
        : view.allowApps !== true ? "turnedOff"
        : view.listsShown !== true ? "loading"
        : searching ? "search" : "lists"

    // Reads the section again while it is on screen.
    function refresh() {
        if (!visible)
            return
        view = JSON.parse(EngineBridge.query("apps.view", ""))
        recentModel.sync(view && view.recent ? view.recent : [], app => app.recentId)
        favoritesModel.sync(view && view.favorites ? view.favorites : [], app => app["package"])
        allModel.sync(view && view.apps ? view.apps : [], app => app["package"])
        refreshSearch()
    }

    function refreshSoon() {
        if (!refreshTimer.running)
            refreshTimer.start()
    }

    // The search's results; read from the field itself, since this runs before the bindings that
    // depend on its text are updated.
    function refreshSearch() {
        const text = search.text.trim()
        const found = searchOffered && text.length > 0 ? JSON.parse(EngineBridge.query("apps.search", text)) || [] : []
        searchModel.sync(found, app => app["package"])
    }

    function perform(name, value) {
        EngineBridge.perform(name, typeof value === "string" ? value : JSON.stringify(value))
    }

    function launch(app) {
        perform("apps.launch", { "package": app["package"] })
    }

    // The search's Return: the first app found opens and the search is cleared.
    function submitSearch() {
        if (!searching || searchModel.count === 0)
            return
        launch(JSON.parse(searchModel.get(0).json))
        search.text = ""
    }

    // The menu of `app` at (x, y) in its tile.
    function showMenu(menu, app, tile, x, y) {
        menu.app = app
        menu.popup(tile, x, y)
    }

    color: Theme.windowBackground
    onVisibleChanged: refresh()
    onQueryChanged: refreshSearch()
    Component.onCompleted: refresh()

    // Lists, icons and notification counts change in bursts: the section is read again at most every
    // 150 ms.
    Timer {
        id: refreshTimer
        interval: 150
        onTriggered: column.refresh()
    }

    Connections {
        target: EngineBridge
        function onAppsRevisionChanged() { column.refreshSoon() }
        function onMirroringRevisionChanged() { column.refreshSoon() }
        function onNotificationsJsonChanged() { column.refreshSoon() }
    }

    KeyedListModel {
        id: recentModel
    }

    KeyedListModel {
        id: favoritesModel
    }

    KeyedListModel {
        id: allModel
    }

    KeyedListModel {
        id: searchModel
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        RowLayout {
            Layout.fillWidth: true
            Layout.preferredHeight: 52
            Layout.leftMargin: 20
            Layout.rightMargin: 14

            Text {
                Layout.fillWidth: true
                text: L10n.t("Apps")
                color: Theme.text
                font.pixelSize: Theme.headlineSize
                font.weight: Font.Bold
            }

            CapsuleField {
                id: search
                Layout.preferredWidth: 260
                visible: column.searchOffered && column.mode !== "unavailable" && column.mode !== "turnedOff"
                placeholderText: L10n.t("Search Apps")
                submittable: false
                Accessible.name: L10n.t("Search Apps")
                Keys.onReturnPressed: event => {
                    column.submitSearch()
                    event.accepted = true
                }
                Keys.onEnterPressed: event => {
                    column.submitSearch()
                    event.accepted = true
                }
                Keys.onEscapePressed: event => {
                    event.accepted = text.length > 0
                    text = ""
                }
            }
        }

        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true

            ColumnLayout {
                anchors.centerIn: parent
                width: Math.min(parent.width - 40, 440)
                visible: column.mode === "unavailable" || column.mode === "turnedOff"
                spacing: 16

                EmptyState {
                    Layout.fillWidth: true
                    iconName: "apps"
                    title: column.mode === "turnedOff" ? L10n.t("Apps Are Turned Off") : L10n.t("Apps Aren't Available")
                    description: column.mode === "turnedOff"
                        ? L10n.f("Turn them on to open %@'s screen and apps on this computer.", [column.phoneName])
                        : L10n.f("%@ doesn't open its apps on this computer.", [column.phoneNameAtStart])
                }

                PillButton {
                    Layout.alignment: Qt.AlignHCenter
                    visible: column.mode === "turnedOff"
                    prominent: true
                    text: L10n.t("Turn On")
                    onClicked: column.perform("apps.allowApps", "")
                }
            }

            ColumnLayout {
                anchors.centerIn: parent
                width: Math.min(parent.width - 40, 440)
                visible: column.mode === "loading"
                spacing: 12

                BusyIndicator {
                    Layout.alignment: Qt.AlignHCenter
                    running: column.mode === "loading" && column.visible
                }

                Text {
                    Layout.fillWidth: true
                    text: L10n.f("Getting apps from %@...", [column.phoneName])
                    color: Theme.secondaryText
                    font.pixelSize: Theme.bodySize
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                }
            }

            EmptyState {
                anchors.centerIn: parent
                width: Math.min(parent.width - 40, 440)
                visible: column.mode === "search" && searchModel.count === 0
                iconName: "search"
                title: L10n.f("No Results for “%@”", [column.query])
                description: L10n.t("Check the spelling or try a new search.")
                Accessible.role: Accessible.AlertMessage
                Accessible.name: title
            }

            Flickable {
                id: scroller
                anchors.fill: parent
                visible: column.mode === "lists" || (column.mode === "search" && searchModel.count > 0)
                contentWidth: width
                contentHeight: groups.implicitHeight + 32
                boundsBehavior: Flickable.StopAtBounds
                clip: true
                ScrollBar.vertical: ScrollBar {}

                ColumnLayout {
                    id: groups
                    x: 20
                    y: 8
                    width: scroller.width - 40
                    spacing: 28

                    AppsGroup {
                        Layout.fillWidth: true
                        visible: column.mode === "search"
                        title: L10n.t("Apps")
                        model: searchModel
                        delegate: AppsTile {
                            id: found
                            required property string json
                            app: JSON.parse(json)
                            onActivated: column.launch(app)
                            onMenuRequested: (x, y) => column.showMenu(appMenu, app, found, x, y)
                        }
                    }

                    AppsGroup {
                        Layout.fillWidth: true
                        visible: column.mode === "lists" && column.view.showsRecent === true
                        title: L10n.t("Recent")
                        hint: L10n.f("Apps you use on %@ appear here.", [column.phoneName])
                        model: recentModel
                        delegate: AppsTile {
                            id: recentTile
                            required property string json
                            app: JSON.parse(json)
                            onActivated: column.perform("apps.openRecent", { recentId: app.recentId })
                            onMenuRequested: (x, y) => column.showMenu(recentMenu, app, recentTile, x, y)
                        }
                    }

                    AppsGroup {
                        Layout.fillWidth: true
                        visible: column.mode === "lists"
                        title: L10n.t("Favorites")
                        hint: L10n.t("Drag apps here to keep them at hand.")
                        acceptsDrops: true
                        model: favoritesModel
                        onAppDropped: name => column.perform("apps.dropOnFavorites", { "package": name, index: null })
                        delegate: AppsTile {
                            id: favoriteTile
                            required property string json
                            required property int index
                            app: JSON.parse(json)
                            draggable: true
                            dropTarget: true
                            onActivated: column.launch(app)
                            onMenuRequested: (x, y) => {
                                favoriteMenu.index = index
                                favoriteMenu.favoriteCount = favoritesModel.count
                                column.showMenu(favoriteMenu, app, favoriteTile, x, y)
                            }
                            onAppDropped: name => column.perform("apps.dropOnFavorites", { "package": name, index: index })
                        }
                    }

                    AppsGroup {
                        Layout.fillWidth: true
                        visible: column.mode === "lists"
                        title: L10n.t("All Apps")
                        hint: L10n.f("Apps from %@ appear here.", [column.phoneName])
                        acceptsDrops: true
                        model: allModel
                        onAppDropped: name => column.perform("apps.dropOnAllApps", name)
                        delegate: AppsTile {
                            id: appTile
                            required property string json
                            app: JSON.parse(json)
                            draggable: true
                            onActivated: column.launch(app)
                            onMenuRequested: (x, y) => column.showMenu(appMenu, app, appTile, x, y)
                        }
                    }
                }
            }
        }
    }

    // An app's menu (every app, search results): open it, or add it to or remove it from the favorites.
    AppsMenu {
        id: appMenu

        property var app: ({})

        AppsMenuItem {
            text: L10n.t("Open")
            onTriggered: column.launch(appMenu.app)
        }

        AppsMenuSeparator {}

        AppsMenuItem {
            text: appMenu.app.favorite === true ? L10n.t("Remove from Favorites") : L10n.t("Add to Favorites")
            onTriggered: column.perform(appMenu.app.favorite === true ? "apps.removeFavorite" : "apps.addFavorite",
                                        appMenu.app["package"])
        }
    }

    // A favorite's menu: open it, move it within the favorites, or remove it.
    AppsMenu {
        id: favoriteMenu

        property var app: ({})
        property int index: -1
        property int favoriteCount: 0

        AppsMenuItem {
            text: L10n.t("Open")
            onTriggered: column.launch(favoriteMenu.app)
        }

        AppsMenuSeparator {}

        AppsMenuItem {
            text: L10n.t("Move Left")
            enabled: favoriteMenu.index > 0
            onTriggered: column.perform("apps.moveFavorite", { "package": favoriteMenu.app["package"], index: favoriteMenu.index - 1 })
        }

        AppsMenuItem {
            text: L10n.t("Move Right")
            enabled: favoriteMenu.index < favoriteMenu.favoriteCount - 1
            onTriggered: column.perform("apps.moveFavorite", { "package": favoriteMenu.app["package"], index: favoriteMenu.index + 1 })
        }

        AppsMenuSeparator {}

        AppsMenuItem {
            text: L10n.t("Remove from Favorites")
            onTriggered: column.perform("apps.removeFavorite", favoriteMenu.app["package"])
        }
    }

    // A recent app's menu: open it from the phone, open its document here or in the browser, and — for
    // the app itself — add it to or remove it from the favorites.
    AppsMenu {
        id: recentMenu

        property var app: ({})

        AppsMenuItem {
            text: L10n.t("Open")
            onTriggered: column.perform("apps.launch", { "package": recentMenu.app["package"],
                                                          taskId: recentMenu.app.mainActivity ? null : recentMenu.app.taskId })
        }

        AppsMenuSeparator {
            visible: recentMenu.app.desktop === true || recentMenu.app.browser === true
        }

        AppsMenuItem {
            visible: recentMenu.app.desktop === true
            text: L10n.t("Open on This Computer")
            onTriggered: column.perform("apps.openDocument", { recentId: recentMenu.app.recentId, where: "desktop" })
        }

        AppsMenuItem {
            visible: recentMenu.app.browser === true
            text: L10n.t("Open in Browser")
            onTriggered: column.perform("apps.openDocument", { recentId: recentMenu.app.recentId, where: "browser" })
        }

        AppsMenuSeparator {
            visible: recentMenu.app.mainActivity === true
        }

        AppsMenuItem {
            visible: recentMenu.app.mainActivity === true
            text: recentMenu.app.favorite === true ? L10n.t("Remove from Favorites") : L10n.t("Add to Favorites")
            onTriggered: column.perform(recentMenu.app.favorite === true ? "apps.removeFavorite" : "apps.addFavorite",
                                        recentMenu.app["package"])
        }
    }
}
