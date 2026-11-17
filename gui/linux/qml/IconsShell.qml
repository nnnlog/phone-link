pragma Singleton
import QtQuick

// Icon paths (24 px viewBox, SVG path data) for this part of the app, merged into Icon.qml.
QtObject {
    readonly property var paths: ({
        "checkmark": "M5 12.5l4.5 4.5L19 7.5",
        "hotspot": "M10.5 13.5l3-3 M9 11l-2.5 2.5a3.2 3.2 0 0 0 4.5 4.5L13.5 15.5 M15 13l2.5-2.5a3.2 3.2 0 0 0-4.5-4.5L10.5 8.5",
        "phoneScreen": "M8 3h8a1.5 1.5 0 0 1 1.5 1.5v15A1.5 1.5 0 0 1 16 21H8a1.5 1.5 0 0 1-1.5-1.5v-15A1.5 1.5 0 0 1 8 3z M9.5 7h2v2h-2z M12.5 7h2v2h-2z M9.5 11h2v2h-2z M12.5 11h2v2h-2z",
        "ringPhone": "M9.5 4h5v16h-5z M6 8a6 6 0 0 0 0 8 M18 8a6 6 0 0 1 0 8 M3.5 6a9.5 9.5 0 0 0 0 12 M20.5 6a9.5 9.5 0 0 1 0 12",
        "sliders": "M4 7h9 M17 7h3 M15 5v4 M4 17h3 M11 17h9 M9 15v4"
    })
}
