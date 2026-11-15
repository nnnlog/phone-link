import QtQuick
import QtQuick.Shapes

Item {
    id: icon

    property string name
    property color color: Theme.text
    property real size: 16
    property bool filled: false

    readonly property var paths: ({
        "bell": "M6 16.5V11a6 6 0 0 1 12 0v5.5l1.5 1.5h-15z M10 20.5a2 2 0 0 0 4 0",
        "bellSlash": "M6 16.5V11a6 6 0 0 1 12 0v5.5l1.5 1.5h-15z M10 20.5a2 2 0 0 0 4 0 M3.5 3.5l17 17",
        "message": "M12 4c4.97 0 9 3.13 9 7s-4.03 7-9 7c-1.1 0-2.15-.15-3.13-.43L4 20l1.4-3.74C3.9 14.9 3 13.03 3 11c0-3.87 4.03-7 9-7z",
        "phone": "M5.5 3.5h3l2 5-2.5 1.5a11 11 0 0 0 6 6l1.5-2.5 5 2v3a2 2 0 0 1-2 2A16 16 0 0 1 3.5 5.5a2 2 0 0 1 2-2z",
        "person": "M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18z M12 7a3 3 0 1 0 0 6a3 3 0 1 0 0-6z M6.5 18.2a6.5 6.5 0 0 1 11 0",
        "personFill": "M12 4a4 4 0 1 0 0 8a4 4 0 1 0 0-8z M4 20.5a8 8 0 0 1 16 0z",
        "photo": "M3.5 5.5h17v13h-17z M3.5 15.5l5-5 4 4 3-3 5 5 M15.5 8.5a.5.5 0 1 0 .01 0",
        "apps": "M4 4h6v6H4z M14 4h6v6h-6z M4 14h6v6H4z M14 14h6v6h-6z",
        "moon": "M19.5 14.5A8 8 0 0 1 9.5 4.5a8 8 0 1 0 10 10z",
        "vibrate": "M8.5 4h7v16h-7z M5 8v8 M19 8v8",
        "speaker": "M4 9h4l5-4v14l-5-4H4z M16.5 8.5a5 5 0 0 1 0 7 M19 6a8.5 8.5 0 0 1 0 12",
        "keyboard": "M3 6.5h18v11H3z M6.5 10h1 M10 10h1 M13.5 10h1 M17 10h1 M7.5 14h9",
        "tablet": "M3.5 6h17v12h-17z M18 12h.01",
        "handset": "M8 3.5h8v17H8z M11 17.5h2",
        "speakerSlash": "M4 9h4l5-4v14l-5-4H4z M16.5 9.5l5 5 M21.5 9.5l-5 5",
        "findPhone": "M9.5 4h5v16h-5z M6 8a6 6 0 0 0 0 8 M18 8a6 6 0 0 1 0 8",
        "pin": "M9 3.5h6l-1 5.5 3 3H7l3-3z M12 12v8.5",
        "close": "M6 6l12 12 M18 6L6 18",
        "compose": "M4 20h4L19 9l-4-4L4 16z M13.5 6.5l4 4",
        "send": "M12 18.5V6 M6.5 11.5L12 6l5.5 5.5",
        "back": "M15 5l-7 7 7 7",
        "chevronDown": "M7 10l5 5 5-5",
        "device": "M8 3h8a1.5 1.5 0 0 1 1.5 1.5v15A1.5 1.5 0 0 1 16 21H8a1.5 1.5 0 0 1-1.5-1.5v-15A1.5 1.5 0 0 1 8 3z M11 18h2",
        "warning": "M12 3.5L2.5 20h19z M12 9.5v5 M12 17.2v.3",
        "gear": "M12 8.5a3.5 3.5 0 1 0 0 7a3.5 3.5 0 1 0 0-7z M12 2.5v3 M12 18.5v3 M2.5 12h3 M18.5 12h3 M5.3 5.3l2.1 2.1 M16.6 16.6l2.1 2.1 M5.3 18.7l2.1-2.1 M16.6 7.4l2.1-2.1",
        "upload": "M7 3.5h7l4 4v13H7z M12 17V10 M9 12.5l3-3 3 3",
        "play": "M7 4.5v15l12-7.5z",
        "pause": "M7 4.5h3.5v15H7z M13.5 4.5H17v15h-3.5z",
        "forward": "M3.5 5.5v13l8-6.5z M12.5 5.5v13l8-6.5z",
        "backward": "M20.5 5.5v13l-8-6.5z M11.5 5.5v13l-8-6.5z",
        "phoneIn": "M5.5 3.5h3l2 5-2.5 1.5a11 11 0 0 0 6 6l1.5-2.5 5 2v3a2 2 0 0 1-2 2A16 16 0 0 1 3.5 5.5a2 2 0 0 1 2-2z M20.5 3.5l-5 5 M15.5 4.5v4h4",
        "phoneOut": "M5.5 3.5h3l2 5-2.5 1.5a11 11 0 0 0 6 6l1.5-2.5 5 2v3a2 2 0 0 1-2 2A16 16 0 0 1 3.5 5.5a2 2 0 0 1 2-2z M15.5 8.5l5-5 M16.5 3.5h4v4",
        "save": "M12 4v11 M7.5 10.5L12 15l4.5-4.5 M4.5 19.5h15",
        "wifi":"M2.5 9a14 14 0 0 1 19 0 M5.5 12.5a9.5 9.5 0 0 1 13 0 M8.8 16a5 5 0 0 1 6.4 0 M12 19.3v.2"
    })

    implicitWidth: size
    implicitHeight: size

    Shape {
        width: 24
        height: 24
        scale: icon.size / 24
        transformOrigin: Item.TopLeft
        preferredRendererType: Shape.CurveRenderer

        ShapePath {
            strokeColor: icon.filled ? "transparent" : icon.color
            fillColor: icon.filled ? icon.color : "transparent"
            strokeWidth: 1.8
            capStyle: ShapePath.RoundCap
            joinStyle: ShapePath.RoundJoin

            PathSvg {
                path: icon.paths[icon.name] || ""
            }
        }
    }
}
