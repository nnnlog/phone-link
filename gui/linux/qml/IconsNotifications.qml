pragma Singleton
import QtQuick

// Icon paths (24 px viewBox, SVG path data) for this part of the app, merged into Icon.qml.
QtObject {
    readonly property var paths: ({
        "bellBadge": "M6 16.5V11a6 6 0 0 1 12 0v5.5l1.5 1.5h-15z M10 20.5a2 2 0 0 0 4 0 M19.5 2.8a2.2 2.2 0 1 0 0 4.4a2.2 2.2 0 1 0 0-4.4z",
        "pinSlash": "M9 3.5h6l-1 5.5 3 3H7l3-3z M12 12v8.5 M4.5 4.5l15 15"
    })
}
