import QtQuick
import QtQuick.Controls.Basic
import PhoneLink

// A context menu in the app's look, of CallMenuItem and CallMenuSeparator entries, as wide as its
// widest entry. `popup()` opens it at the pointer.
Menu {
    id: menu

    padding: 5
    width: {
        let widest = 200
        for (let index = 0; index < count; index++) {
            const entry = itemAt(index)
            if (entry && entry.visible)
                widest = Math.max(widest, entry.implicitWidth + leftPadding + rightPadding)
        }
        return Math.min(widest, 480)
    }

    background: Rectangle {
        implicitWidth: 200
        radius: Theme.rowRadius
        color: Theme.raisedBackground
        border.color: Theme.separator
    }
}
