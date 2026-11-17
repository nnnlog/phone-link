import AppKit
import Observation
import os
import PhoneLinkEngine

private let log = AppLog(category: "phone-files")

/// The phone's storage in the Finder, as Settings shows it: the switch, the permission the phone
/// needs, the latest sync, the phone's free space, Refresh, Show in Finder, and the notice that
/// deleted files go to the phone's recycle bin.
@MainActor
@Observable
public final class PhoneFilesModel {
    /// `nil` until the engine has started (and while no phone is selected).
    public private(set) var state: PhoneFilesState?
    /// The last action of the user's that failed, for an alert; the view clears it.
    public var failure: SettingsFailure?
    /// Deleted files went to the phone's recycle bin: tell the user once (they may turn it off).
    public var recycleNoticeShown = false
    /// The phone was asked for the permission and is prompting.
    public private(set) var permissionRequested = false

    private let api: any PhoneFilesApiProtocol
    private let finderURL: @MainActor () async -> URL?
    private let open: @MainActor (URL) -> Void

    public convenience init(api: any PhoneFilesApiProtocol, hub: EngineEventHub, domains: PhoneFilesDomains) {
        self.init(api: api, hub: hub, finderURL: { await domains.userVisibleURL() }, open: { NSWorkspace.shared.open($0) })
    }

    init(
        api: any PhoneFilesApiProtocol,
        hub: EngineEventHub,
        finderURL: @escaping @MainActor () async -> URL?,
        open: @escaping @MainActor (URL) -> Void
    ) {
        self.api = api
        self.finderURL = finderURL
        self.open = open

        hub.onEvent { [weak self] event in
            guard case let .phoneFiles(change) = event else {
                if case .resync(.phoneFiles) = event { self?.reload() }
                return
            }
            switch change {
            case .stateChanged, .running:
                self?.reload()
            case .recycleBinNotice:
                if self?.state?.recycleNoticeOff == false { self?.recycleNoticeShown = true }
            case .itemsChanged:
                break
            }
        }
        hub.onStart { [weak self] in self?.reload() }
    }

    public func reload() {
        do {
            state = try api.state()
            if state?.permissionGranted == true { permissionRequested = false }
        } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
            state = nil
        } catch {
            log.error("phone-files state not read: \(error: error)")
        }
    }

    /// The phone serves its files to this Mac: the section shows.
    public var available: Bool { state?.capable == true }

    /// The user's switch.
    public var enabled: Bool {
        get { state?.enabled ?? false }
        set {
            do {
                try api.setEnabled(on: newValue)
            } catch {
                log.error("phone-files setting not changed: \(error: error)")
                failure = SettingsFailure(title: String(localized: "Couldn't Change the Setting"), message: UserText.describe(error))
            }

            reload()
        }
    }

    /// The feature runs but the phone has not granted access to its files.
    public var needsPermission: Bool { state?.running == true && state?.permissionGranted == false }

    /// Asks the phone for access to its files (it prompts).
    public func requestPermission() async {
        do {
            switch try await api.requestPermission(origin: .statusUi) {
            case .granted:
                permissionRequested = false
            case .required:
                permissionRequested = true
            case let .failed(reason):
                log.error("phone-files permission not requested: \(reason, privacy: .public)")
                failure = SettingsFailure(title: String(localized: "Couldn't Ask Your Phone"), message: String(localized: "Make sure your phone is nearby and connected, then try again."))
            }
        } catch {
            log.error("phone-files permission not requested: \(error: error)")
            failure = SettingsFailure(title: String(localized: "Couldn't Ask Your Phone"), message: UserText.describe(error))
        }

        reload()
    }

    /// What the latest sync did, for the status line.
    public var syncStatus: String? {
        guard let state, state.running else { return nil }
        if state.ltwOff { return String(localized: "Link to Windows is turned off on your phone.") }
        switch state.sync {
        case .idle: return state.initialSyncDone ? String(localized: "Up to date") : String(localized: "Waiting for your phone…")
        case .preparing, .syncing: return String(localized: "Updating…")
        case .succeeded: return String(localized: "Up to date")
        case .connectionFailed: return String(localized: "Couldn't reach your phone.")
        case .failed: return String(localized: "Couldn't update the files from your phone.")
        }
    }

    /// The phone's free and total space, as the phone counts gigabytes.
    public var storageStatus: String? {
        guard let state, state.running, state.storageTotal > 0 else { return nil }
        let gb = Double(max(state.gigabyteBytes, 1))
        let free = Double(state.storageFree) / gb
        let total = Double(state.storageTotal) / gb
        return String(localized: "\(String(format: "%.1f", free)) GB free of \(String(format: "%.1f", total)) GB")
    }

    /// The user's refresh: the phone checks its storage for changes.
    public func refresh() {
        do {
            try api.refresh()
        } catch {
            log.error("phone files not refreshed: \(error: error)")
            failure = SettingsFailure(title: String(localized: "Couldn't Refresh"), message: UserText.describe(error))
        }
    }

    /// Opens the phone's folder in the Finder.
    public func showInFinder() async {
        guard let url = await finderURL() else {
            failure = SettingsFailure(title: String(localized: "Couldn't Open the Folder"), message: String(localized: "Your phone's files aren't in the Finder yet."))
            return
        }
        open(url)
    }

    /// The user read the recycle-bin notice; `again: false` turns it off.
    public func dismissRecycleNotice(again: Bool) {
        recycleNoticeShown = false
        guard !again else { return }
        do {
            try api.setRecycleNoticeOff()
        } catch {
            log.error("recycle-bin notice not turned off: \(error: error); it shows again")
        }

        reload()
    }

    /// Where the phone keeps deleted files, for the notice.
    public var recycleBinName: String {
        state?.recycleBin.map { $0.split(separator: "\\").last.map(String.init) ?? $0 } ?? String(localized: "Recycle bin")
    }
}
