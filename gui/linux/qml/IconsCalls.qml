pragma Singleton
import QtQuick

// Icon paths (24 px viewBox, SVG path data) for this part of the app, merged into Icon.qml.
QtObject {
    readonly property var paths: ({
        // A handset put down: a declined call, ending a call.
        "phoneDown": "M2.5 13.2C7.5 8.4 16.5 8.4 21.5 13.2l-1.6 3.1-3.9-1.3-.4-2.9a11 11 0 0 0-7.2 0l-.4 2.9-3.9 1.3z",
        // A handset with waves: calls placed and answered from this computer.
        "phoneConnection": "M5.5 3.5h3l2 5-2.5 1.5a11 11 0 0 0 6 6l1.5-2.5 5 2v3a2 2 0 0 1-2 2A16 16 0 0 1 3.5 5.5a2 2 0 0 1 2-2z M14.5 3.5a6.5 6.5 0 0 1 6 6 M14.5 7a3 3 0 0 1 2.5 2.5",
        // An antenna crossed out: no connection to the phone.
        "antennaSlash": "M12 11.5v9 M8.6 8.1a4.8 4.8 0 0 0 0 6.8 M15.4 8.1a4.8 4.8 0 0 1 0 6.8 M5.8 5.3a8.8 8.8 0 0 0 0 12.4 M18.2 5.3a8.8 8.8 0 0 1 0 12.4 M3.5 3.5l17 17",
        // Nine keys: the keypad.
        "keypad": "M6 4.8a1.2 1.2 0 1 0 0 2.4a1.2 1.2 0 1 0 0-2.4z M12 4.8a1.2 1.2 0 1 0 0 2.4a1.2 1.2 0 1 0 0-2.4z M18 4.8a1.2 1.2 0 1 0 0 2.4a1.2 1.2 0 1 0 0-2.4z M6 10.8a1.2 1.2 0 1 0 0 2.4a1.2 1.2 0 1 0 0-2.4z M12 10.8a1.2 1.2 0 1 0 0 2.4a1.2 1.2 0 1 0 0-2.4z M18 10.8a1.2 1.2 0 1 0 0 2.4a1.2 1.2 0 1 0 0-2.4z M6 16.8a1.2 1.2 0 1 0 0 2.4a1.2 1.2 0 1 0 0-2.4z M12 16.8a1.2 1.2 0 1 0 0 2.4a1.2 1.2 0 1 0 0-2.4z M18 16.8a1.2 1.2 0 1 0 0 2.4a1.2 1.2 0 1 0 0-2.4z",
        // Delete the last character.
        "deleteLeft": "M9 5.5h10.5a1 1 0 0 1 1 1v11a1 1 0 0 1-1 1H9l-6-6.5z M12.5 9.5l5 5 M17.5 9.5l-5 5"
    })
}
