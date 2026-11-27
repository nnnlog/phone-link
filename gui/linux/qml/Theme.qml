pragma Singleton
import QtQuick

QtObject {
    readonly property bool dark: Qt.styleHints.colorScheme !== undefined && Qt.styleHints.colorScheme === Qt.ColorScheme.Dark

    readonly property color windowBackground: dark ? "#1e1e1e" : "#ffffff"
    readonly property color listBackground: dark ? "#232323" : "#f7f7f7"
    readonly property color sidebarBackground: dark ? "#2c2c2e" : "#e8e8ed"
    readonly property color sidebarModule: dark ? Qt.rgba(1, 1, 1, 0.09) : Qt.rgba(1, 1, 1, 0.55)
    readonly property color separator: dark ? Qt.rgba(1, 1, 1, 0.1) : Qt.rgba(0, 0, 0, 0.1)
    readonly property color selection: dark ? Qt.rgba(1, 1, 1, 0.1) : Qt.rgba(0, 0, 0, 0.07)
    readonly property color hover: dark ? Qt.rgba(1, 1, 1, 0.05) : Qt.rgba(0, 0, 0, 0.035)
    readonly property color text: dark ? "#f5f5f7" : "#1d1d1f"
    readonly property color secondaryText: dark ? "#98989d" : "#6e6e73"
    readonly property color accent: dark ? "#0a84ff" : "#007aff"
    readonly property color controlBackground: dark ? Qt.rgba(1, 1, 1, 0.1) : Qt.rgba(0, 0, 0, 0.05)
    readonly property color controlHover: dark ? Qt.rgba(1, 1, 1, 0.16) : Qt.rgba(0, 0, 0, 0.09)
    readonly property color fieldBackground: dark ? Qt.rgba(1, 1, 1, 0.05) : "#ffffff"
    readonly property color fieldBorder: dark ? Qt.rgba(1, 1, 1, 0.14) : Qt.rgba(0, 0, 0, 0.14)
    readonly property color incomingBubble: dark ? "#3a3a3c" : "#e9e9eb"
    readonly property color outgoingBubble: accent
    readonly property color warning: "#ff9f0a"
    readonly property color destructive: "#ff453a"
    readonly property color raisedBackground: dark ? "#2c2c2e" : "#ffffff"

    // Palette roles the Basic controls draw with (BusyIndicator, CheckBox, Slider, ScrollBar, ToolTip,
    // Popup); every window sets them, since a window does not inherit another window's palette.
    readonly property color paletteLight: dark ? "#636366" : "#ffffff"
    readonly property color paletteMidlight: dark ? "#48484a" : "#d1d1d6"
    readonly property color paletteMid: dark ? "#8e8e93" : "#aeaeb2"
    readonly property color paletteDark: dark ? "#aeaeb2" : "#6e6e73"
    readonly property color toolTipBackground: dark ? "#3a3a3c" : "#ffffff"

    readonly property int sidebarWidth: 320
    readonly property int listWidth: 320
    readonly property int moduleRadius: 12
    readonly property int rowRadius: 8
    readonly property int edge: 10
    readonly property int content: 19

    readonly property int titleSize: 22
    readonly property int headlineSize: 17
    readonly property int bodySize: 13
    readonly property int subheadSize: 12
    readonly property int captionSize: 11
}
