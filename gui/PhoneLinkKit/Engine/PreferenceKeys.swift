/// `UserDefaults` keys of the app's own preferences (the engine keeps the features' settings).
public enum PreferenceKeys {
    /// Whether the menu bar extra is shown (HIG: the user decides; default on).
    public static let showMenuBarExtra = "showMenuBarExtra"
    /// Whether the main window opens when the app launches (default on).
    public static let showWindowAtLaunch = "showWindowAtLaunch"
    /// The Settings window's selected pane.
    public static let settingsPane = "settingsPane"
    /// Every app seen in the phone's notifications, package name → display name, so an app turned
    /// off (whose notifications the list no longer shows) keeps its row in Settings.
    public static let notificationSettingsApps = "notificationSettingsApps"
    /// Whether the phone's wallpaper is the sidebar's background (default on).
    public static let useWallpaperAsBackground = "useWallpaperAsBackground"
    /// Whether the phone's media shows in macOS's Now Playing and answers the media keys (default off).
    public static let useNowPlaying = "useNowPlaying"
    /// Whether the app checks for updates once a day (default on).
    public static let updatesCheckAutomatically = "updatesCheckAutomatically"
    /// The update channel, `stable` or `beta` (default `stable`).
    public static let updatesChannel = "updatesChannel"
    /// When the app last read its releases.
    public static let updatesLastChecked = "updatesLastChecked"
    /// The release version the user skipped.
    public static let updatesSkippedVersion = "updatesSkippedVersion"
    /// The user read, on this Mac, that calls placed and answered here stay on the phone (shown the
    /// first time call control is switched on).
    public static let callControlIntroShown = "callControlIntroShown"
}
