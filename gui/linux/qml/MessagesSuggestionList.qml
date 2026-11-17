import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import PhoneLink

// The phone's contacts matching what is typed in "To:"; a click adds one as a recipient.
ListView {
    id: suggestionList

    property var suggestions: []

    // A suggestion was added: the "To:" field takes the focus back.
    signal added()

    clip: true
    model: suggestions
    topMargin: 6
    bottomMargin: 6
    leftMargin: 10
    rightMargin: 10
    ScrollBar.vertical: ScrollBar {}

    delegate: AbstractButton {
        id: suggestion
        required property var modelData
        required property int index
        width: ListView.view.width - 20
        implicitHeight: 44
        hoverEnabled: true
        Accessible.role: Accessible.Button
        Accessible.name: modelData.name
        Accessible.description: modelData.line
        onClicked: {
            EngineBridge.perform("messages.addSuggestion", String(index))
            suggestionList.added()
        }

        background: Rectangle {
            radius: Theme.rowRadius
            color: suggestion.hovered || suggestion.visualFocus ? Theme.hover : "transparent"
        }

        contentItem: RowLayout {
            spacing: 10

            Avatar {
                Layout.leftMargin: 6
                name: suggestion.modelData.name
                photo: suggestion.modelData.photo || ""
                size: 28
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 1

                Text {
                    Layout.fillWidth: true
                    text: suggestion.modelData.name
                    color: Theme.text
                    font.pixelSize: Theme.bodySize + 1
                    elide: Text.ElideRight
                }

                Text {
                    Layout.fillWidth: true
                    text: suggestion.modelData.line
                    color: Theme.secondaryText
                    font.pixelSize: Theme.captionSize
                    elide: Text.ElideRight
                }
            }
        }
    }
}
