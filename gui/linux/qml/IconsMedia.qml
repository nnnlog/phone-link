pragma Singleton
import QtQuick

// Icon paths (24 px viewBox, SVG path data) for this part of the app, merged into Icon.qml.
QtObject {
    readonly property var paths: ({
        // The mirror image of chevronRight.
        "chevronLeft": "M14 6l-6 6 6 6",
        "computer": "M3.5 5h17v11h-17z M9 20h6 M12 16v4",
        "globe": "M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18z M3.5 12h17 M12 3c2.4 2.5 3.6 5.5 3.6 9s-1.2 6.5-3.6 9 M12 3c-2.4 2.5-3.6 5.5-3.6 9s1.2 6.5 3.6 9",
        "arrowUpForward": "M7.5 16.5l9-9 M9.5 7.5h7v7",
        "openExternal": "M13.5 4.5h6v6 M19.5 4.5l-8 8 M17.5 14v5.5h-13v-13H10",
        "copy": "M9 9h10.5v10.5H9z M15 9V4.5H4.5V15H9",
        "arrowDown": "M12 5.5V18 M6.5 12.5L12 18l5.5-5.5",
        "exclamation": "M12 7v6.5 M12 16.8v.2",
        "phoneSlash": "M8 3h8a1.5 1.5 0 0 1 1.5 1.5v15A1.5 1.5 0 0 1 16 21H8a1.5 1.5 0 0 1-1.5-1.5v-15A1.5 1.5 0 0 1 8 3z M3.5 4l17 16",
        "musicNote": "M10 17.5V4.5c2.2 1.8 5.8 2.2 6.5 5.5 M10 17.5a2.75 2.75 0 1 1-5.5 0a2.75 2.75 0 1 1 5.5 0z"
    })
}
