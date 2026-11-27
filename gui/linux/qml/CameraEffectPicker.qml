import QtQuick
import QtQuick.Controls.Basic
import PhoneLink

// The camera's effect picker: a pop-up button with the effect in use, its list the phone's effects.
// Picking one tells the window (`picked`); the phone's answer then moves the selection.
ComboBox {
    id: control

    property int selectedIndex: 0
    signal picked(int index)

    currentIndex: selectedIndex
    font.pixelSize: Theme.bodySize
    leftPadding: 12
    rightPadding: 30
    Accessible.name: L10n.t("Effect")
    onActivated: index => {
        // The list shows the phone's selection until the phone applies the new one.
        currentIndex = Qt.binding(() => control.selectedIndex)
        if (index !== selectedIndex)
            picked(index)
    }

    contentItem: Text {
        text: control.displayText
        font: control.font
        color: Theme.text
        opacity: control.enabled ? 1 : 0.4
        verticalAlignment: Text.AlignVCenter
        elide: Text.ElideRight
    }

    indicator: Icon {
        x: control.width - width - 10
        y: (control.height - height) / 2
        name: "chevronDown"
        size: 14
        color: Theme.secondaryText
    }

    background: Rectangle {
        implicitWidth: 120
        implicitHeight: 28
        radius: height / 2
        color: control.hovered ? Theme.controlHover : Theme.controlBackground
        border.color: control.visualFocus ? Theme.accent : "transparent"
        opacity: control.down ? 0.75 : 1
    }

    delegate: ItemDelegate {
        required property var modelData
        required property int index
        width: ListView.view ? ListView.view.width : implicitWidth
        highlighted: control.highlightedIndex === index
        font.pixelSize: Theme.bodySize

        contentItem: Text {
            text: parent.modelData
            font: parent.font
            color: Theme.text
            elide: Text.ElideRight
            verticalAlignment: Text.AlignVCenter
        }

        background: Rectangle {
            radius: Theme.rowRadius
            color: parent.highlighted ? Theme.selection : "transparent"
        }
    }

    // Its own window, as a menu is, so that the list is not cut by the small window it opens from. Qt
    // before 6.8 has no popupType and keeps the list inside the window.
    popup: Popup {
        id: list
        Component.onCompleted: {
            if (list.popupType !== undefined)
                list.popupType = Popup.Window
        }
        y: control.height + 4
        width: Math.max(control.width, 180)
        implicitHeight: contentItem.implicitHeight + 8
        padding: 4

        contentItem: ListView {
            clip: true
            implicitHeight: contentHeight
            model: control.popup.visible ? control.delegateModel : null
            currentIndex: control.highlightedIndex
        }

        background: Rectangle {
            radius: Theme.moduleRadius
            color: Theme.raisedBackground
            border.color: Theme.separator
        }
    }
}
