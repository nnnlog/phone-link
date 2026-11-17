pragma Singleton
import QtQuick

// Icon paths (24 px viewBox, SVG path data) for this part of the app, merged into Icon.qml.
QtObject {
    readonly property var paths: ({
        "lock": "M6.5 11h11v9h-11z M8.5 11V8a3.5 3.5 0 0 1 7 0v3",
        "pauseCircle": "M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18z M10 9v6 M14 9v6",
        "appClose": "M6 4h12a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z M9.5 9.5l5 5 M14.5 9.5l-5 5",
        "thermometer": "M10 5a2 2 0 0 1 4 0v8.5a4 4 0 1 1-4 0z M12 10v6",
        "batteryLow": "M3.5 8h15v8h-15z M21 10.5v3 M6.5 11v2",
        "battery": "M3.5 8h15v8h-15z M21 10.5v3 M6.5 11v2 M9.5 11v2",
        "batteryCharging": "M3.5 8h15v8h-15z M21 10.5v3 M12 9.5l-2.5 3h3l-2.5 3",
        "handRaised": "M8.5 12.5V7a1.5 1.5 0 0 1 3 0v4.5 M11.5 11V5a1.5 1.5 0 0 1 3 0v6 M14.5 11V7a1.5 1.5 0 0 1 3 0v6.5a7 7 0 0 1-7 7h-.5a6 6 0 0 1-5-2.7L3.7 15a1.5 1.5 0 0 1 2.5-1.7l2.3 2.7",
        "computerPhone": "M3 6h11v8H3z M1.5 17h14 M17 8.5h5v11h-5z M19 17.5h1",
        "layoutSwitch": "M9 5.5h6v13H9z M2.5 12h4 M4.5 10l-2 2 2 2 M21.5 12h-4 M19.5 10l2 2-2 2",
        "phoneLock": "M15.5 9V4.5A1.5 1.5 0 0 0 14 3H6.5A1.5 1.5 0 0 0 5 4.5v15A1.5 1.5 0 0 0 6.5 21H11 M13.5 15h7v6h-7z M15 15v-1.5a2 2 0 0 1 4 0V15",
        "wifiWarning": "M2.5 9a14 14 0 0 1 15-3 M5.5 12.5a9.5 9.5 0 0 1 9.5-2.5 M8.8 16a5 5 0 0 1 5-1 M20 10v5 M20 18v.3",
        "handTap": "M9.5 12V6.5a1.5 1.5 0 0 1 3 0V12 M12.5 11a1.5 1.5 0 0 1 3 0v1 M15.5 11.5a1.5 1.5 0 0 1 3 0v3a6.5 6.5 0 0 1-6.5 6.5h-.5a5.5 5.5 0 0 1-4.6-2.5l-2-3a1.4 1.4 0 0 1 2.3-1.6l1.8 2.1 M7 4.5a5 5 0 0 1 8 0",
        "speakerQuiet": "M4 9h4l5-4v14l-5-4H4z",
        "recents": "M7 7h12v12H7z M4 16V4h12"
    })
}
