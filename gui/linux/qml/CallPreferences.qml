pragma Singleton
import Qt.labs.settings

// The calls' preferences the app keeps itself, rather than the engine.
Settings {
    category: "Calls"

    // Whether a call placed or answered on the phone is told about with a notification that offers
    // to transfer it to this computer.
    property bool phoneCallNotices: true
}
