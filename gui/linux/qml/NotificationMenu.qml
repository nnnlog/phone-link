import QtQuick
import QtQuick.Controls.Basic
import PhoneLink

// A menu of the notifications part, in the app's colors, filled when it opens from a list of entries:
// { text, onTriggered, enabled, chosen, destructive }, { separator: true }, or { text, submenu: [...] }.
// The keyboard moves through it (arrows, Return, Escape) as through any menu.
Menu {
    id: menu

    // The submenus made for the current entries.
    property var submenus: []
    property Component itemComponent: NotificationMenuItem {}
    property Component separatorComponent: MenuSeparator {
        topPadding: 4
        bottomPadding: 4
        leftPadding: 10
        rightPadding: 10

        contentItem: Rectangle {
            implicitHeight: 1
            color: Theme.separator
        }
    }

    function clearEntries() {
        while (count > 0) {
            const item = takeItem(0)
            if (item)
                item.destroy()
        }
        submenus.forEach(submenu => submenu.destroy())
        submenus = []
    }

    function fill(entries) {
        clearEntries()
        entries.forEach(entry => {
            if (entry.separator) {
                addItem(separatorComponent.createObject(null))
            } else if (entry.submenu) {
                const submenu = Qt.createComponent("NotificationMenu.qml").createObject(menu.parent, { title: entry.text })
                submenu.fill(entry.submenu)
                submenus = submenus.concat([submenu])
                addMenu(submenu)
            } else {
                const item = itemComponent.createObject(null, {
                    text: entry.text,
                    enabled: entry.enabled !== false,
                    chosen: entry.chosen === true,
                    destructive: entry.destructive === true
                })
                item.triggered.connect(entry.onTriggered)
                addItem(item)
            }
        })
        // As wide as the widest command, up to a width past which commands elide.
        let widest = 160
        for (let i = 0; i < count; i++)
            widest = Math.max(widest, itemAt(i).implicitWidth)
        width = Math.min(widest, 380) + leftPadding + rightPadding
    }

    // Opens with `entries` at the pointer, or at (x, y) in `item` (from the keyboard).
    function show(entries, item, x, y) {
        fill(entries)
        if (item)
            popup(item, x, y)
        else
            popup()
    }

    topPadding: 5
    bottomPadding: 5
    leftPadding: 0
    rightPadding: 0
    overlap: 6
    delegate: NotificationMenuItem {}

    background: Rectangle {
        implicitWidth: 160
        radius: 8
        color: Theme.raisedBackground
        border.color: Theme.separator
    }
}
