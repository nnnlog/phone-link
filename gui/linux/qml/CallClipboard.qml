import QtQuick

// Copies text to the clipboard: QML has no clipboard of its own, so a hidden text edit copies it.
Item {
    function copy(text) {
        edit.text = text
        edit.selectAll()
        edit.copy()
        edit.text = ""
    }

    visible: false

    TextEdit {
        id: edit
    }
}
