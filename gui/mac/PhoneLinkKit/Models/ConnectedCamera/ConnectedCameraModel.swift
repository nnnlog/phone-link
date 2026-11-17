import AppKit
import Observation
import os
import PhoneLinkEngine

private let log = AppLog(category: "camera")

/// The phone's camera as a camera on this Mac: the camera extension's state, the setting, and the
/// camera control window's state and actions for the selected phone.
@MainActor
@Observable
public final class ConnectedCameraModel {
    /// `nil` until the engine has started (and while no phone is selected).
    public private(set) var status: PhoneCameraStatus?
    public private(set) var extensionState: CameraExtensionActivator.State = .notRequested
    /// The phone whose camera turning this one on replaced, for a note.
    public private(set) var replacedPhone: String?
    /// The settings' permission request runs.
    public private(set) var requestingPermission = false
    /// The last action of the user's that failed, for an alert; the view clears it.
    public var failure: SettingsFailure?

    /// Shows (`true`) or hides the camera control window.
    public var presentControlWindow: ((Bool) -> Void)?

    private let api: any ConnectedCameraApiProtocol
    private let activator: CameraExtensionActivator
    private let camera: PhoneCameraAdapter
    @ObservationIgnored private var waitingForProvider: Task<Void, Never>?

    public init(api: any ConnectedCameraApiProtocol, hub: EngineEventHub, activator: CameraExtensionActivator, camera: PhoneCameraAdapter) {
        self.api = api
        self.activator = activator
        self.camera = camera

        camera.onClientsChanged = { [api] deviceId, streaming in
            do {
                try api.clientChanged(deviceId: deviceId, started: streaming)
            } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
                log.debug("camera client change dropped; no phone session")
            } catch {
                log.error("camera client change not passed on: \(error: error)")
            }
        }

        activator.onChange = { [weak self] state in
            self?.extensionChanged(state)
        }
        hub.onEvent { [weak self] event in
            self?.handle(event)
        }
        hub.onStart { [weak self] in
            self?.availabilityChanged()
        }
    }

    /// Activates the camera extension (at launch, and from the setting's retry).
    public func activateExtension() {
        activator.activate()
    }

    private func extensionChanged(_ state: CameraExtensionActivator.State) {
        extensionState = state
        camera.setExtensionActive(state == .active)
        availabilityChanged()

        waitingForProvider?.cancel()
        guard state == .active, !camera.isAvailable() else { return }

        // A newly activated extension shows up in CoreMediaIO a moment later.
        waitingForProvider = Task { [weak self, camera] in
            for _ in 0 ..< 30 {
                try? await Task.sleep(for: .seconds(1))
                guard !Task.isCancelled else { return }
                if camera.isAvailable() {
                    self?.availabilityChanged()
                    return
                }
            }
            log.error("activated camera extension not found in CoreMediaIO after 30 s; the camera stays unavailable")
        }
    }

    private func availabilityChanged() {
        do {
            try api.availabilityChanged()
        } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
            // The start tells the engine.
        } catch {
            log.error("camera availability not passed on: \(error: error)")
        }
        reload()
    }

    /// The camera setting shows: the phone can be a camera.
    public var showsSetting: Bool { status?.supported == true }

    /// "Use as a camera".
    public var cameraEnabled: Bool {
        get { status?.cameraEnabled ?? false }
        set { perform(String(localized: "Couldn't Change the Setting"), "camera setting not changed") { try $0.setCameraEnabled(on: newValue) } }
    }

    public func retryRegistration() {
        perform(String(localized: "Couldn't Turn On the Camera"), "camera registration not retried") { try $0.retryRegistration() }
    }

    /// The setting's "Allow" for the camera permission: the phone shows its prompt.
    public func requestPermission() async {
        requestingPermission = true
        defer { requestingPermission = false }
        do {
            _ = try await api.settingsRequestPermission()
        } catch {
            log.error("camera permission not requested: \(error: error)")
            failure = SettingsFailure(title: String(localized: "Couldn't Ask Your Phone"), message: UserText.describe(error))
        }

        reload()
    }

    /// Opens System Settings where the camera extension is allowed.
    public func openExtensionSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.LoginItems-Settings.extension") {
            NSWorkspace.shared.open(url)
        }
    }

    // MARK: - The control window

    public func pause() { perform(String(localized: "Couldn't Pause"), "camera not paused") { try $0.pause() } }
    public func resume() { perform(String(localized: "Couldn't Resume"), "camera not resumed") { try $0.resume() } }

    public func switchCamera() {
        let face: PhoneCameraFace = status?.face == .front ? .back : .front
        perform(String(localized: "Couldn't Switch Cameras"), "camera face not switched") { try $0.switchCamera(face: face) }
    }

    public func selectEffect(_ index: Int) {
        perform(String(localized: "Couldn't Apply the Effect"), "camera effect not applied") { try $0.selectEffect(index: UInt32(index)) }
    }

    /// The error screen's action.
    public func errorAction() {
        switch status?.errorAction {
        case .refresh: perform(String(localized: "Couldn't Reconnect"), "camera not reconnected") { try $0.refresh() }
        case .requestPermissions: perform(String(localized: "Couldn't Ask Your Phone"), "camera permissions not requested") { try $0.requestPermissions() }
        case .close: perform(String(localized: "Couldn't Close"), "camera error not closed") { try $0.closeFatalError() }
        case .none?, nil: break
        }
    }

    /// The user closed the window: the stream closes.
    public func closeControlWindow() {
        perform(String(localized: "Couldn't Close"), "camera control window not closed") { try $0.closeControlWindow() }
    }

    public func reload() {
        do {
            status = try api.status(locale: Self.effectNameLocale(localization: Bundle.main.preferredLocalizations.first, current: .current))
        } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
            status = nil
        } catch {
            log.error("camera status not read: \(error: error)")
        }
    }

    private func perform(_ title: String, _ subject: StaticString, _ action: (any ConnectedCameraApiProtocol) throws -> Void) {
        do {
            try action(api)
        } catch {
            log.error("\(subject.description, privacy: .public): \(error: error)")
            failure = SettingsFailure(title: title, message: UserText.describe(error))
        }
        reload()
    }

    private func handle(_ event: EngineEvent) {
        switch event {
        case .connectedCamera(.changed), .resync(feature: .connectedCamera), .phoneChanged:
            reload()
        case let .connectedCamera(.controlWindow(shown)):
            reload()
            presentControlWindow?(shown)
        case let .connectedCamera(.replaced(name)):
            replacedPhone = name
            reload()
        case .accountRemoved, .phoneUnlinked:
            status = nil
        default:
            break
        }
    }

    // MARK: - Texts

    /// What the control window says for the current error.
    public static func errorText(_ error: PhoneCameraError) -> (title: String, message: String) {
        switch error {
        case .channelDisconnected, .disconnected:
            (String(localized: "Your phone is disconnected"), String(localized: "Make sure your phone is nearby and connected, then reconnect."))
        case .meteredConnection:
            (String(localized: "Your phone is on mobile data"), String(localized: "Link to Windows doesn't use mobile data on your phone. Connect it to Wi-Fi or allow mobile data, then reconnect."))
        case .ltwDisabled:
            (String(localized: "Link to Windows is turned off"), String(localized: "Turn on Link to Windows on your phone, then reconnect."))
        case .postNotificationPermissionNeeded:
            (String(localized: "Allow notifications on your phone"), String(localized: "Scan the code with your phone to allow Link to Windows to show notifications, then reconnect."))
        case .cameraPermissionNeeded, .cameraPermissionNeededAutoprompt:
            (String(localized: "Allow camera access on your phone"), String(localized: "Link to Windows needs your phone's camera. Allow it on your phone."))
        case .cameraPermissionDenied:
            (String(localized: "Camera access was denied"), String(localized: "Allow Link to Windows to use the camera on your phone, then try again."))
        case .cameraPermissionPermanentlyDenied:
            (String(localized: "Camera access is turned off"), String(localized: "Allow camera access for Link to Windows in your phone's settings, then try again."))
        case .cameraPermissionRevoked:
            (String(localized: "Camera access was removed"), String(localized: "Allow camera access for Link to Windows on your phone."))
        case .screenLocked:
            (String(localized: "Your phone is locked"), String(localized: "Unlock your phone to continue."))
        case .callInProgress:
            (String(localized: "A call is in progress"), String(localized: "The camera continues after the call."))
        case .deviceHeat:
            (String(localized: "Your phone is too warm"), String(localized: "Let your phone cool down, then try again."))
        case .cameraInUse, .maxCamerasInUse:
            (String(localized: "Your phone's camera is in use"), String(localized: "Close the other app using the camera on your phone."))
        case .cameraDeviceFatal, .cameraServiceFatal, .sessionConfigurationFailed:
            (String(localized: "Your phone's camera stopped"), String(localized: "Restart the camera on your phone, then try again."))
        case .cameraDisabled, .noCameraAvailable:
            (String(localized: "Your phone's camera isn't available"), String(localized: "Check that the camera works on your phone."))
        case .channelNotCreated:
            (String(localized: "Unlock this Mac"), String(localized: "The camera connects once this Mac is unlocked."))
        case .none, .unknown, .other:
            (String(localized: "Something went wrong"), String(localized: "Try again."))
        }
    }

    /// The locale whose names the phone's own effects are shown in: the system's, unless the app runs in
    /// another language (`localization`, when it has no translation for the system's), then that one.
    nonisolated static func effectNameLocale(localization: String?, current: Locale) -> String {
        guard let localization else { return current.identifier(.bcp47) }
        let app = Locale(identifier: localization)
        return app.language.languageCode == current.language.languageCode ? current.identifier(.bcp47) : app.identifier(.bcp47)
    }

    /// The label of effect `effect`.
    public static func effectLabel(_ effect: PhoneCameraEffect) -> String {
        switch effect {
        case .none: label(.noFilter)
        case .automatic: label(.automatic)
        case .bokeh: label(.blur)
        case .faceRetouch: label(.faceRetouch)
        case .hdr: label(.hdr)
        case .night: label(.night)
        case .oem(.known(let key), _): label(key)
        case .oem(.text(let text), _): text
        }
    }

    private static func label(_ key: EffectLabel) -> String {
        switch key {
        case .noFilter: String(localized: "No Effect", comment: "Camera effect")
        case .automatic: String(localized: "Automatic", comment: "Camera effect")
        case .blur: String(localized: "Blur", comment: "Camera effect")
        case .faceRetouch: String(localized: "Face Retouch", comment: "Camera effect")
        case .hdr: String(localized: "HDR", comment: "Camera effect")
        case .night: String(localized: "Night", comment: "Camera effect")
        case .autoFraming: String(localized: "Auto-Framing", comment: "Camera effect")
        case .softFocus: String(localized: "Soft Focus", comment: "Camera effect")
        case .stabilization: String(localized: "Stabilization", comment: "Camera effect")
        }
    }
}
