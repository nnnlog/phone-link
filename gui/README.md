# `gui/` — the macOS app

The native macOS side: a SwiftUI app that links the Rust engine ([`engine/`](../engine/README.md))
in-process through its `uniffi` bindings, implements the engine's OS edges with macOS frameworks, and shows
the features. Toolchain: Xcode (run `xcodebuild` through the `justfile`, which sets
`DEVELOPER_DIR`) and stable Rust.

## Build and test

Every command is a recipe in the [`justfile`](../justfile) at the repository root (`just` lists them):

| Recipe | Does |
|---|---|
| `just gui-build` / `just gui-build Release` | `xcodebuild build` of scheme `PhoneLink` into `build/DerivedData` |
| `just gui-test` | `xcodebuild test` (Debug) |
| `just build-release` | the Release build, signed as `Signing.local.xcconfig` names (for this Mac) |
| `just dmg` | that build packaged by `scripts/make-dmg.sh` as `build/PhoneLink-<version>.dmg` (the version is `MARKETING_VERSION`) |
| `just distribute` | the archive re-signed with Developer ID (`ExportOptions-DeveloperID.plist`) and packaged in `build/developer-id/`; not notarized |
| `just distribute-notarized` | the same, notarized by `scripts/notarize.sh` (`ExportOptions-DeveloperID-Notarize.plist`), in `build/notarized/` |
| `just gui-strings` | the Debug build, then `scripts/sync-strings.sh`: brings `PhoneLink/Localizable.xcstrings` up to date with the strings the code uses (*Localization*) |
| `just gui-strings-check` | `scripts/check-strings.sh`: fails when a string catalog lacks a translation for a language the project lists (part of `just check`) |

The first build phase (`scripts/build-engine.sh`, in the `PhoneLinkEngine` target) runs `cargo build` for
the configuration's profile (Debug → `debug`, Release → `release`) and regenerates the bindings into
`Generated/` (gitignored), replacing a file only when it changed. The library builds in
`engine/target/app/` and the bindgen binary in `target/bindgen/`, apart from `target/debug/`: cargo
names a static library without a hash, so a build with other features in the same directory —
`cargo test`'s dev-dependency features, the bindgen binary's `bindgen-cli` — would replace the one the
app links. Cargo runs with a clean environment, so Xcode's exported settings do not reach it. The app
links the engine's static library (`libphonelink_engine.a`) and `libiconv`, with no App Sandbox, under the
hardened runtime that notarization requires (the entitlements allow location, which the hotspot's Wi-Fi
scan needs). Its File Provider extension runs sandboxed; the app and the extension share the app
group `$(TeamIdentifierPrefix)$(PHONELINK_BUNDLE_ID)`, which is also the extension's document group.
`Signing.xcconfig` signs it ad-hoc unless a gitignored `Signing.local.xcconfig` beside it names a
code-signing identity and team (`CODE_SIGN_IDENTITY = Apple Development`, `DEVELOPMENT_TEAM`).

`Version.xcconfig` holds the version and build number for every target. The version
(`MARKETING_VERSION`) follows [Semantic Versioning](https://semver.org), and the engine's `Cargo.toml`
has the same one. The build number (`CURRENT_PROJECT_VERSION`) goes up by one for every build that
leaves the Mac.

Version `0.2.0` gets tag `v0.2.0` and a GitHub release. A beta such as `0.3.0-beta.1` or `0.3.0-rc.1`
gets a pre-release, which only users who turned on betas see. The updater skips a release until
`PhoneLink-<version>.dmg` is attached. It installs the new app only if the build number is higher and the
Developer ID team matches the running app. If not, it opens the release page.

`Identity.xcconfig` sets `PHONELINK_UPDATE_REPOSITORY`, the repository the updater reads and the About
panel links to. Leave it empty to turn the updater off and drop the links. It also sets `PHONELINK_BUNDLE_ID`, the base of every bundle identifier and app group. Each
bundle's `Info.plist` carries it as `PhoneLinkBundleID`, which `PhoneLinkShared/PhoneLinkIdentity` reads
at run time. Changing it starts the keychain item, data, log and camera device over.

Every test is offline: `PhoneLinkKitTests` runs without a host app, against fakes of the engine's
protocols, and never builds a live `Engine`. Running the app signs in to the Microsoft account and wakes the
phone.

## Targets

| Target | Kind | Holds |
|---|---|---|
| `PhoneLinkEngine` | static library, Swift 5 mode | the generated bindings (`Generated/phonelink_engine.swift`), module `PhoneLinkEngine` |
| `PhoneLinkKit` | static library, Swift 6 | `Engine/` (the engine's controller, event hub, navigator, user-facing texts), `Edges/` (the OS-edge adapters), `Models/` (one `@Observable` model per feature) |
| `PhoneLink` | app, Swift 6, `LSUIElement` | `App/` (the app, the app delegate that builds everything), `Views/` (one folder per feature); embeds `PhoneFilesProvider` |
| `PhoneFilesProvider` | File Provider extension (`com.apple.fileprovider-nonui`), Swift 5 mode, App Sandbox | `PhoneFilesProvider/`: the phone's storage as a Finder domain. It holds no phone state: every call goes to the app over the extension's File Provider service (an XPC connection the app opens while the domain exists), and content crosses it as open file handles. The system replaces extension processes on its own, so a process without a connection posts the distributed notification `<bundle id>.phone-files.needs-host` and the app connects again. Its `Info.plist` and entitlements are `PhoneFilesProvider-Info.plist` and `PhoneFilesProvider.entitlements` |
| `PhoneCameraExtension` | Camera Extension (a system extension), App Sandbox | `PhoneCameraExtension/`: the phone's camera as a macOS camera device; the app feeds it frames. Its `Info.plist` and entitlements are `PhoneCameraExtension-Info.plist` and `PhoneCameraExtension.entitlements` |
| `PhoneLinkKitTests` | unit tests (Swift Testing), no host; `PhoneLinkKitTests-Info.plist` | adapters and models |

The folders are file-system-synchronized groups: a file added to a folder belongs to its target without
editing the project. `PhoneFilesShared/` (the app–extension XPC contract) belongs to both `PhoneLinkKit` and
`PhoneFilesProvider`, `PhoneCameraShared/` (the app–camera-extension contract) to both `PhoneLinkKit`
and `PhoneCameraExtension`, and `PhoneLinkShared/` (the bundle identifier) to `PhoneLinkKit` and both extensions.

## Icons

Every icon is generated from one geometry by `scripts/make_icons.py` (`just icons`, which also exports
`Design/AppIcon.png`); edit the script, not its outputs:

| Output | Used as |
|---|---|
| `PhoneLink/App/AppIcon.icon` | the app icon (Icon Composer document, `ASSETCATALOG_COMPILER_APPICON_NAME = AppIcon`); `actool` compiles it into `Assets.car` and an `AppIcon.icns` for macOS 14–15 |
| `PhoneLink/Assets.xcassets/MenuBarIcon` | the menu bar extra's label, a template image with its vector kept |
| `Design/AppIcon.svg` | both layers flat on the white tile, without the system's Liquid Glass, for reference and export; not built |
| `Design/AppIcon.png` | the app icon as macOS renders it (Liquid Glass, 512 px), exported from `AppIcon.icon` by Icon Composer's `ictool`; shown by the repository's README; not built |

## Localization

The app is in English (the source and fallback language) and Korean, and follows the language macOS
picks for it from the user's preferred languages; it has no language setting of its own.

| Catalog | Holds |
|---|---|
| `PhoneLink/Localizable.xcstrings` | every string of the UI, from `PhoneLink/` and `PhoneLinkKit/`; the keys are the English text |
| `PhoneLink/InfoPlist.xcstrings` | the app's `Info.plist` usage descriptions (the English ones are the `INFOPLIST_KEY_*` build settings) |
| `PhoneCameraExtension/InfoPlist.xcstrings` | the camera extension's `NSSystemExtensionUsageDescription` |

- **Where text is localized.** A SwiftUI literal (`Text("…")`, `Button("…")`, `.help("…")`) is already a
  key. Every other text of ours is written as `String(localized: "…")` where the literal is: `PhoneLinkKit`'s
  texts (it is a static library, so the lookup goes to the app's bundle), AppKit titles and messages, and
  notification content. Values go inside the key as interpolations, so a translation can reorder them;
  counts use one key with plural variations rather than a `count == 1` branch.
- **What is not.** Text the phone or the engine sends — names, message and notification bodies, app names,
  an OEM camera effect's own name, an engine error's `reason` — is shown as received (`Text(String)`,
  `Text(verbatim:)`). Log messages are not localized.
- **Syncing.** Xcode syncs a catalog only from its own target's code, and `xcodebuild` never syncs it, so
  `PhoneLinkKit`'s strings would read as stale there. `just gui-strings` is the sync: it reads the
  `.stringsdata` the compiler writes for both targets (`SWIFT_EMIT_LOC_STRINGS`). Run it after changing
  text, then translate the new keys.
- **Wording.** Labels whose meaning comes from the phone use the words the phone and Phone Link on Windows
  use in that language: the camera effects, and the phone app's name (*Link to Windows*, in Korean
  "Windows와 연결"). The rest follows macOS's own terms in that language. Korean follows the
  [korean-prose](https://github.com/JellyBrick/korean-prose-skill) style: 해요체 for sentences and alert titles
  ("사진을 저장하지 못했어요", "다시 시도해 주세요"), noun forms for buttons, menu items and status labels, no
  dual particles such as 을(를) after a name the app fills in (the sentence is phrased around it instead), and no
  em or en dashes.
- **Adding a language.** Add it to `knownRegions` in `PhoneLink.xcodeproj`, then translate until
  `just gui-strings-check` passes.

## OS edges

`SystemEdges.live(keychainService:)` builds one adapter per foreign trait of the engine
([`engine/README.md`](../engine/README.md)) and the app delegate hands them to `Engine`. The engine
calls them synchronously on its own threads, so no adapter assumes the main thread; shared state sits
behind `OSAllocatedUnfairLock`. The rules the adapters follow are in the trait and adapter doc
comments.

| Engine trait | Adapter (`PhoneLinkKit/`) | macOS source |
|---|---|---|
| `SecureStore` | `Edges/KeychainStore` | one generic-password item in the login keychain — service = bundle id, account `engine-store` — holding every key as a binary property list; read once, written through |
| `SessionLock` | `Edges/SessionLockMonitor` | `CGSessionCopyCurrentDictionary()` › `CGSSessionScreenIsLocked`, read on every call |
| `HostInfo` | `Edges/HostInfoAdapter` | `gethostname` (as the CLI), `Locale.current.region` when it is a two-letter code |
| `Reachability` | `Edges/NetworkPathMonitor` | one `NWPathMonitor` for the app's lifetime, shared with the PC status |
| `EventSink` | `Engine/EngineEventHub` (`EventBridge`) | hops to the main actor and fans out to the controller and the models |
| `SystemNotifier` | `Edges/UserNotifier` | `UNUserNotificationCenter` (*Notifications*, below) |
| `ClipboardPasteboard` | `Edges/PasteboardAdapter` | `NSPasteboard.general` |
| `ClipboardImageCodec` | `Edges/ImageIOCodec` | ImageIO decoding, PNG/TIFF encoding, Core Graphics resampling |
| `PcStatusSource` | `Edges/PcStatusAdapter` | IOKit power sources, `ProcessInfo.isLowPowerModeEnabled`, the shared path monitor, CoreWLAN RSSI |
| `PcPowerControl` | `Edges/PowerControlAdapter` | lock: `SACLockScreenImmediate` of the private `login.framework`, looked up when called (no public call locks the session at once); sleep: IOKit `IOPMSleepSystem`; shut down / restart: the Apple event `kAEShutDown` / `kAERestart` to `loginwindow`, without a reply. Its tests replace every call, so none acts on the machine |
| `PcWallpaperSource` | `Edges/PcWallpaperAdapter` | `NSWorkspace.desktopImageURL(for:)` of the first screen (read on the main thread at each request; no screen listed is a failed read), ImageIO decoding, Core Graphics resampling to the engine's `wallpaperScaledSize`, JPEG at quality 0.9 |
| `AttachmentImaging` | `Edges/AttachmentImager` | ImageIO thumbnails; per size the source format when ImageIO writes it (every frame of a GIF), then JPEG at decreasing quality, shrinking until the byte budget fits |
| `MessagingFileTypes` | `Edges/UTTypeFileTypes` | `UTType` conformance: vCard, image, audio, movie, text, document, in that order |
| `PhoneProximity` | `Edges/BluetoothProximity` | IOBluetooth: controller power, the first paired device named like the phone |
| `PhoneBluetooth` | `Edges/PhoneBluetoothAdapter` | IOBluetooth on the main thread for every feature that reaches the phone over Bluetooth: controller power, the first paired device named like the phone, `IOBluetoothDeviceInquiry` and `IOBluetoothDevicePair` (the numeric comparison confirmed in an alert and answered through CoreBluetooth's private pairing agent), RFCOMM channels from the service record the Mac holds for the paired phone |
| `HotspotWifi` | `Edges/HotspotWifiAdapter` | CoreWLAN: the interface, a scan for the SSID, WPA personal association, the current network's SSID, disassociation; its SSID and link events reload `HotspotModel` |

### Notifications

One `UNNotificationCategory` per distinct action layout, identified by the SHA-256 of the layout's
length-prefixed action fields, so equal layouts share a category within and across runs. Every
notification gets a category with `customDismissAction`, also one without actions, so every dismissal
reaches the engine. Registering categories replaces the app's whole set, so the set keeps every layout
this run registered and, from launch, the categories of notifications still shown from an earlier run.
An alert is posted with the default sound; a silent delivery with interruption level `passive`, which
stays in the list (no banner) also while the app is in front. Authorization counts when it is
`authorized` or `provisional`, read from the center at every query. A post and a query wait on the
engine's thread for the center's answer, at most 2 s: a refused or unanswered post throws (the engine then
records nothing as posted), an unanswered query counts as not allowed. An image the notification center
does not accept is left out and the notification is still posted.

In the window, a messaging item offers Open in Messages in its detail and context menu (the Messages
section opens). The post-notification stage's Show Me How… opens a sheet with the link as a QR code and
as text; the sheet closes itself when the stage ends.

### System events

| macOS | Engine |
|---|---|
| `NSApplication` did-become-active / did-resign-active (a window or the menu bar extra's menu is in front) | `setForeground(true)` / `setForeground(false)` |
| `NSWorkspace.willSleepNotification`, distributed `com.apple.screenIsLocked` | `systemEvent(.willSleep)`, `systemEvent(.screenLocked)` |
| `NSWorkspace.didWakeNotification`, distributed `com.apple.screenIsUnlocked` | `systemEvent(.didWake)`, `systemEvent(.screenUnlocked)` |
| a notification response | `notificationResponse(id:userInfo:response:)`, held until a start has finished (the click that launched the app) and dropped once the account was removed |
| Quit | `stop()`, waited for at most 3 s |

The two distributed notifications are observed with suspension behavior `deliverImmediately`: AppKit
suspends distributed notifications while the app is not active, which a menu bar app nearly always is.

### Files to and from the phone

The send window (the phone menu's Send Files to Phone…) takes dropped or chosen files and shows the
send's progress and outcome itself; files dropped on the app or opened with it (`PhoneLink-Info.plist`
declares the app an alternate viewer of `public.data`, so the Finder's Open With lists it) are sent with
notifications reporting progress and outcome. A received notification's click opens the file, or the
folder for several files. The *Files* settings pane holds the phone's *Share* toggle and the
received-files folder.

### Permissions macOS asks for

| Prompt | When |
|---|---|
| Notifications | at first launch (`requestAuthorization`: alert, sound, badge) |
| Keychain access | after a rebuild of an ad-hoc signed app: the signature changes with every build, and the item's access list names the old one; one item means one prompt. A team-signed app keeps its identity across builds and asks once |
| Pasteboard contents (macOS 15.4 and later) | when the clipboard feature first reads the general pasteboard's data; denied ⇒ nothing is sent to the phone |
| Bluetooth | the first proximity check of the messaging gate, or the first use of a feature that reaches the phone over Bluetooth (`NSBluetoothAlwaysUsageDescription`) |

## Log

`LogArchive` installs the engine's log at launch in `~/Library/Logs/<bundle id>/`: `PhoneLink.log` and
two older generations, rotated at 4 MiB, readable only by the user, and `log.key`, the key of the tags.
Every app-process logger is an `AppLog`, which writes each line to the unified log (Console) and to that
file. An interpolated value follows the unified log's privacy rules (a number or Bool is public, any
other value private unless marked `privacy: .public`), and the file holds a private value only as a
`{{REDACTED:private|<tag>}}` token and scrubs a public one of identifier shapes (paths, URLs, GUIDs) the
way the engine scrubs its error texts, because an error's description is public and a Foundation error
quotes the paths it failed on; an error is interpolated as `\(error: error)`, which also leaves out the
credentials a hotspot error carries. Levels, which side logs a failure (the app logs what the engine
returns to it; the engine logs what it handles itself), and the message style match the engine's log,
with `notice` for the engine's `warn`. Settings › Advanced ›
Log shows the log's size, saves a copy where the user chooses and shows it in the Finder, and clears it
(which also replaces the key). The camera and File Provider extensions run in their own processes and log
to the unified log only.

## Features

Each model (`PhoneLinkKit/Models/<Feature>/`) holds what its views show. It reads the engine's feature
object (handed out by `EngineController`) when the engine starts, on the feature's events
(`EngineEventHub`), and after each change the user makes; a `Resync` event re-reads everything. Before the
engine runs, every read fails with `NotStarted`, which no model shows as an error. Models are tested against
fakes of the generated `…ApiProtocol`s.

| Where | Model | Views | Engine API |
|---|---|---|---|
| The sidebar's phone area (status and control tiles), the menu bar extra and the main menu's Phone menu | `DeviceStatusModel`, `HotspotModel` | `Views/Sidebar/PhoneArea`, `Views/MenuBar/PhoneMenu` | `DeviceStatusApi`, `HotspotApi` |
| Notifications section | `NotificationsModel` | `Views/Notifications/` | `NotificationsApi` |
| Messages section | `MessagesModel` | `Views/Messages/` | `MessagingApi`, `ContactsApi` |
| Calls section (call history; Settings › Messages › Calls) | `CallsModel` | `Views/Calls/`, `Views/Settings/SettingsMessagesPane` | `CallingApi` |
| Contacts section | `ContactsModel` | `Views/Contacts/` | `ContactsApi` |
| Photos section | `PhotosModel` | `Views/Photos/` | `PhotosApi` |
| What plays on the phone: the menu bar extra, the main menu's Phone menu, the bottom of the sidebar, macOS's Now Playing (opt-in); the General pane's media switches | `MediaControlsModel` | `Views/MenuBar/MediaControlItems`, `Views/Sidebar/SidebarActivity`, `Views/Settings/SettingsMediaControlsRows` | `AudioApi` |
| The sidebar's phone picture and background; the General pane's background setting | `WallpaperModel` | `Views/Sidebar/PhoneArea`, `Views/Shared/PhoneWallpaper`, `Views/Settings/SettingsGeneralPane` | `WallpaperApi` |
| Phone screen and app windows; the Phone Screen pane | `ScreenMirroringModel` | `Views/Mirroring/`, `Views/Settings/SettingsMirroringPane` | `ScreenMirroringApi` |
| Apps section | `PhoneAppsModel` | `Views/Apps/` | `PhoneAppsApi` |
| The camera window; the Camera pane | `ConnectedCameraModel` | `Views/Camera/`, `Views/Settings/SettingsCameraPane` | `ConnectedCameraApi` |
| The send window, received-file notifications; the Files pane | `FileTransferModel` | `Views/FileTransfer/`, `Views/Settings/SettingsFilesPane` | `FileTransferApi` |
| The phone's storage in the Finder (`PhoneFilesProvider`) | `PhoneFilesModel` | `Views/Settings/SettingsPhoneFilesSection` | `PhoneFilesApi` |
| New-photo notifications; the Photos pane | `RemoteSnippingModel` | `Views/Settings/SettingsPhotosPane` | `RemoteSnippingApi` |
| The phone locking, sleeping, restarting or shutting down the Mac | `RemoteControlModel` | `Views/Settings/SettingsRemoteControlRow` | `RemoteControlApi` |
| QR and PIN pairing of a new phone | `PairingModel` | `Views/Pairing/` | `PairingApi` |
| Settings window | `SettingsModel` | `Views/Settings/` | the features' APIs |
| Main window shell, sign-in window | `EngineController`, `AppNavigator` | `Views/MainWindow`, `Views/SignIn/` | `Engine`, `PhoneApi` |

A click on a system notification reaches the engine, which answers with the owning feature's event:
`OpenRequested` shows the notification (a messaging notification: the Messages section; a phone call's: the Calls section), `OpenConversation` the conversation, `OpenImage` opens the photo
in its default app, `LowBatteryNotificationActivated` opens Settings, and `BringWindowForward` brings the
main window to the front.

### Local choices

Decisions the protocol leaves to the app are in the doc comments of the model or view that makes them.

### Not surfaced

The engine keeps these; the app has no UI for them:

- a contacts browser (contacts serve names, photos and recipient suggestions; the new-message composer
  and Settings › Messages show the contacts prompt instead);
- the birthday banner;
- separate MMS receive and send switches (both default on);
- the per-phone Cross Device switch;
- the clipboard's global switch (default on; `ClipboardApi.set_global_switch`);
- a banner's own per-app menu (`NotificationsApi.set_app_local_setting`: a macOS banner has no menu);
- the multi-app notification preferences dialog (`NotificationsApi.apply_preferences`: Settings changes
  one app at a time).
