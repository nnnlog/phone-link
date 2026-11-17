import Foundation
import os
import SystemExtensions

private let log = AppLog(category: "camera")

/// Installs and activates the app's camera extension, a system extension: the first activation waits
/// for the user's approval in System Settings; a newer build replaces the active one.
@MainActor
public final class CameraExtensionActivator: NSObject {
    public enum State: Equatable, Sendable {
        case notRequested
        case activating
        /// The user allows it in System Settings › General › Login Items & Extensions.
        case needsApproval
        case active
        /// It runs after the Mac restarts.
        case needsRestart
        case failed(String)
    }

    public private(set) var state: State = .notRequested {
        didSet { onChange?(state) }
    }

    public var onChange: ((State) -> Void)?

    /// Asks the system to activate the extension.
    public func activate() {
        guard state != .activating else { return }
        state = .activating

        let request = OSSystemExtensionRequest.activationRequest(
            forExtensionWithIdentifier: PhoneCameraShared.extensionBundleID,
            queue: .main
        )
        request.delegate = self
        OSSystemExtensionManager.shared.submitRequest(request)
    }
}

extension CameraExtensionActivator: OSSystemExtensionRequestDelegate {
    public nonisolated func request(
        _ request: OSSystemExtensionRequest,
        actionForReplacingExtension existing: OSSystemExtensionProperties,
        withExtension ext: OSSystemExtensionProperties
    ) -> OSSystemExtensionRequest.ReplacementAction {
        .replace
    }

    public nonisolated func requestNeedsUserApproval(_ request: OSSystemExtensionRequest) {
        log.info("camera extension waits for the user's approval")
        MainActor.assumeIsolated { state = .needsApproval }
    }

    public nonisolated func request(_ request: OSSystemExtensionRequest, didFinishWithResult result: OSSystemExtensionRequest.Result) {
        MainActor.assumeIsolated {
            switch result {
            case .completed:
                log.info("camera extension activated")
                state = .active
            case .willCompleteAfterReboot:
                log.info("camera extension activates after a restart")
                state = .needsRestart
            @unknown default:
                log.error("camera extension not activated: unknown result \(result.rawValue)")
                state = .failed(String(localized: "The camera extension couldn't be installed (error \(result.rawValue))."))
            }
        }
    }

    public nonisolated func request(_ request: OSSystemExtensionRequest, didFailWithError error: any Error) {
        log.error("camera extension not activated: \(error: error)")
        MainActor.assumeIsolated { state = .failed(error.localizedDescription) }
    }
}
