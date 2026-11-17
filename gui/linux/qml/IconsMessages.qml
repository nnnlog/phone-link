pragma Singleton
import QtQuick

// Icon paths (24 px viewBox, SVG path data) for this part of the app, merged into Icon.qml.
QtObject {
    readonly property var paths: ({
        "plus": "M12 5v14 M5 12h14",
        "people": "M9 4.5a3 3 0 1 0 0 6a3 3 0 1 0 0-6z M2.5 19.5a6.5 6.5 0 0 1 13 0z M16.5 6a2.6 2.6 0 1 0 0 5.2a2.6 2.6 0 1 0 0-5.2z M15 13.4a5.6 5.6 0 0 1 6.5 6.1h-4.5a8 8 0 0 0-2-6.1z",
        "eyeSlash": "M2.5 12s3.5-6.5 9.5-6.5 9.5 6.5 9.5 6.5-3.5 6.5-9.5 6.5S2.5 12 2.5 12z M12 9a3 3 0 1 0 0 6a3 3 0 1 0 0-6z M4 4l16 16",
        "trash": "M4.5 6.5h15 M9.5 6.5v-2h5v2 M6.5 6.5l1 13h9l1-13 M10 10v6 M14 10v6",
        "sim": "M7 3.5h7l4 4v13H7z M9.5 11h6v6.5h-6z",
        "alertCircle": "M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18z M12 7.5v6 M12 16.3v.4",
        "film": "M4 4.5h16v15H4z M8 4.5v15 M16 4.5v15 M4 9h4 M4 15h4 M16 9h4 M16 15h4",
        "waveform": "M3 12h1.5 M6.5 8v8 M10 5v14 M13.5 9v6 M17 7v10 M20.5 11v2",
        "contactCard": "M3.5 5.5h17v13h-17z M9 9a2.2 2.2 0 1 0 0 4.4a2.2 2.2 0 1 0 0-4.4z M5.8 16.5a3.2 3.2 0 0 1 6.4 0 M14 10h4 M14 13.5h4",
        "docText": "M6.5 3.5h7l4 4v13h-11z M13.5 3.5v4h4 M9 12h6 M9 15.5h6",
        "doc": "M6.5 3.5h7l4 4v13h-11z M13.5 3.5v4h4",
        "search": "M10.5 4a6.5 6.5 0 1 0 0 13a6.5 6.5 0 1 0 0-13z M15.5 15.5l4.5 4.5",
        "chevronRight": "M10 6l6 6-6 6"
    })
}
