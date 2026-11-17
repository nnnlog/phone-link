import QtQuick
import QtQuick.Controls.Basic
import PhoneLink

// A context menu in the app's look (the Photos and Apps sections): a raised rounded panel as wide as
// its widest row, rows that highlight under the pointer and with the arrow keys.
Menu {
    id: menu

    padding: 5
    width: {
        let widest = 160
        for (let index = 0; index < count; index++) {
            const item = itemAt(index)
            if (item && item.visible)
                widest = Math.max(widest, item.implicitWidth)
        }
        return Math.min(widest, 380) + leftPadding + rightPadding
    }
    delegate: AppsMenuItem {}

    background: Rectangle {
        implicitWidth: 170
        radius: Theme.rowRadius + 2
        color: Theme.raisedBackground
        border.color: Theme.separator
    }
}
