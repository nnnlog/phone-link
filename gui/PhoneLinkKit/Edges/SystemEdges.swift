import Foundation
import PhoneLinkEngine

/// The macOS implementations of the engine's OS edges, built once at launch and handed to `Engine`.
/// The engine calls every adapter synchronously on its own threads, so none of them assumes the
/// main thread.
public struct SystemEdges: Sendable {
    public let store: any SecureStore
    public let lock: any SessionLock
    public let host: any HostInfo
    public let reachability: any Reachability
    public let notifier: UserNotifier
    public let pasteboard: any ClipboardPasteboard
    public let imageCodec: any ClipboardImageCodec
    public let pcStatus: any PcStatusSource
    public let attachmentImaging: any AttachmentImaging
    public let fileTypes: any MessagingFileTypes
    public let proximity: any PhoneProximity
    public let pcPower: any PcPowerControl
    public let pcWallpaper: any PcWallpaperSource
    public let phoneBluetooth: any PhoneBluetooth
    public let hotspotWifi: HotspotWifiAdapter
    public let phoneCamera: PhoneCameraAdapter
    public let phoneAppsHost: any PhoneAppsHost
    public let mirrorPresenter: MirrorPresenterAdapter

    /// The adapters of this Mac; `keychainService` names the Keychain item.
    public static func live(keychainService: String) -> SystemEdges {
        // Reachability and the PC status for the phone read the same path monitor.
        let network = NetworkPathMonitor()
        return SystemEdges(
            store: KeychainStore(service: keychainService),
            lock: SessionLockMonitor(),
            host: HostInfoAdapter(),
            reachability: network,
            notifier: UserNotifier(),
            pasteboard: PasteboardAdapter(),
            imageCodec: ImageIOCodec(),
            pcStatus: PcStatusAdapter(network: network),
            attachmentImaging: AttachmentImager(),
            fileTypes: UTTypeFileTypes(),
            proximity: BluetoothProximity(),
            pcPower: PowerControlAdapter(),
            pcWallpaper: PcWallpaperAdapter(),
            phoneBluetooth: PhoneBluetoothAdapter(),
            hotspotWifi: HotspotWifiAdapter(),
            phoneCamera: PhoneCameraAdapter(),
            phoneAppsHost: PhoneAppsHostAdapter(),
            mirrorPresenter: MirrorPresenterAdapter()
        )
    }
}
