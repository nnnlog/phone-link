import CoreImage
import Foundation
import os
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

@MainActor
struct PairingModelTests {
    private let api = FakePairingApi()
    private let hub = EngineEventHub()

    private func model(loadingLimit: Duration = .seconds(35), permissionsLimit: Duration = .seconds(180)) -> PairingModel {
        PairingModel(api: api, hub: hub, loadingLimit: loadingLimit, permissionsLimit: permissionsLimit)
    }

    private func post(_ session: UInt64, _ step: PairingStep) {
        hub.post(.pairing(update: PairingUpdate(session: session, step: step)))
    }

    private static let code = PairingCode(value: "https://aka.ms/qr", expiresAt: 2_000_000_000, expiresInMinutes: 10)
    private static let pin = PairingCode(value: "4821", expiresAt: 2_000_000_000, expiresInMinutes: 5)

    @Test func theQrFlowShowsTheCodeThePinAndSelectsThePairedPhone() {
        let model = model()
        var paired: [String] = []
        model.onPaired = { paired.append($0) }

        model.start()
        #expect(api.calls.withLock { $0.starts } == [.qrCode])
        #expect(model.page == .loading(.qrCode))

        post(1, .waitingForScan(qrCode: Self.code, manualLink: "https://aka.ms/manual"))
        #expect(model.page == .qrCode(Self.code, manualLink: "https://aka.ms/manual"))

        post(1, .waitingForPin(pin: Self.pin, kind: .challenge))
        #expect(model.page == .pin(Self.pin, .challenge))

        post(1, .waitingForPermissions)
        post(1, .completing)
        #expect(model.page == .completing)

        post(1, .ended(result: .success(phoneId: "phone-2")))
        #expect(model.page == .paired)
        #expect(paired == ["phone-2"])
    }

    @Test func stepsOfAnotherSessionAreIgnored() {
        let model = model()
        model.start()
        post(7, .waitingForScan(qrCode: Self.code, manualLink: "x"))
        #expect(model.page == .loading(.qrCode))
    }

    @Test func pairingManuallyKeepsTheQrSessionUntilContinue() {
        let model = model()
        api.accountNameValue = "me@example.com"
        model.start()
        post(1, .waitingForScan(qrCode: Self.code, manualLink: "https://aka.ms/manual"))
        model.showManualInstructions()
        #expect(model.page == .manualInstructions(link: "https://aka.ms/manual", accountName: "me@example.com"))

        // The QR session goes on behind the instructions.
        post(1, .waitingForScan(qrCode: Self.code, manualLink: "https://aka.ms/manual"))
        #expect(model.page == .manualInstructions(link: "https://aka.ms/manual", accountName: "me@example.com"))

        model.backToQrCode()
        #expect(model.page == .qrCode(Self.code, manualLink: "https://aka.ms/manual"))
        #expect(api.calls.withLock { $0.starts } == [.qrCode], "back to the running QR session")

        model.showManualInstructions()
        model.continueManually()
        #expect(api.calls.withLock { $0.starts } == [.qrCode, .manualPin])

        // The QR session's cancel arrives for the old session and changes nothing.
        post(1, .ended(result: .canceled))
        #expect(model.page == .loading(.manualPin))
        post(2, .waitingForPin(pin: Self.pin, kind: .pairing))
        #expect(model.page == .pin(Self.pin, .pairing))
    }

    @Test func anExpiredChallengePinCanBeRefreshed() {
        let model = model()
        model.start()
        post(1, .pinRefresh(pin: Self.pin, kind: .challenge, problem: .expired, refreshing: false, failure: nil))
        model.refreshPin()
        #expect(api.calls.withLock { $0.refreshes } == 1)
        #expect(model.page == .pinProblem(Self.pin, .challenge, .expired, refreshing: true, failure: nil))

        model.refreshPin()
        #expect(api.calls.withLock { $0.refreshes } == 1, "one refresh at a time")

        post(1, .pinRefresh(pin: Self.pin, kind: .challenge, problem: .expired, refreshing: false, failure: .networkUnavailable))
        #expect(model.page == .pinProblem(Self.pin, .challenge, .expired, refreshing: false, failure: .networkUnavailable))
    }

    @Test func theManualPinIsNeverRefreshed() {
        let model = model()
        model.start(.manualPin)
        post(1, .pinRefresh(pin: Self.pin, kind: .pairing, problem: .expired, refreshing: false, failure: nil))
        model.refreshPin()
        #expect(api.calls.withLock { $0.refreshes } == 0)
    }

    @Test func endingsShowTheirPage() {
        let model = model()
        model.start()
        post(1, .ended(result: .sessionExpired))
        #expect(model.page == .failed(.expired))

        model.start()
        post(2, .ended(result: .accountMismatched))
        #expect(model.page == .failed(.ended(.accountMismatched)))
        #expect(PairingModel.Failure.ended(.accountMismatched).message.contains("different Microsoft account"))

        api.startError = FeatureApiError.NotStarted
        model.start()
        #expect(model.page == .failed(.couldNotStart(UserText.describe(FeatureApiError.NotStarted))))
    }

    @Test func aCodeThatDoesNotComeInTimeFailsAndCancelsTheSession() async {
        let model = model(loadingLimit: .milliseconds(50))
        model.start()
        try? await Task.sleep(for: .milliseconds(300))
        #expect(model.page == .failed(.loadingTookTooLong))
        #expect(api.calls.withLock { $0.cancels } == 1)

        post(1, .ended(result: .canceled))
        #expect(model.page == .failed(.loadingTookTooLong))
    }

    @Test func aLongWaitForPermissionsShowsTheErrorPage() async {
        let model = model(permissionsLimit: .milliseconds(50))
        model.start()
        post(1, .waitingForPermissions)
        try? await Task.sleep(for: .milliseconds(300))
        #expect(model.page == .failed(.permissionsTookTooLong))
        #expect(api.calls.withLock { $0.cancels } == 0)
    }

    @Test func cancellingReturnsToIdle() {
        let model = model()
        model.start()
        model.cancel()
        #expect(model.page == .idle)
        #expect(api.calls.withLock { $0.cancels } == 1)

        post(1, .ended(result: .canceled))
        #expect(model.page == .idle)
    }
}

struct PairingQRCodeTests {
    @Test func theCodeEncodesTheLinkVerbatimWithAFourModuleQuietZone() throws {
        let link = "https://aka.ms/qr?code=Ab_9-x&vm=1"
        let image = try #require(PairingQRCode.image(for: link))
        let modules = try #require(PairingQRCode.darkModules(of: image))

        // The symbol is 21 + 4k modules; the image adds four light modules on each side.
        #expect((modules.count - 21) % 4 == 0)
        #expect(image.width == modules.count + 2 * PairingQRCode.quietZone)
        #expect(image.height == image.width)
        // The top-left finder pattern starts right after the quiet zone.
        #expect(modules[0][0] && modules[0][6] && modules[6][0])

        // A scanner reads back exactly the link.
        let scaled = CIImage(cgImage: image).transformed(by: CGAffineTransform(scaleX: 8, y: 8))
        let detector = try #require(CIDetector(ofType: CIDetectorTypeQRCode, context: nil, options: nil))
        let decoded = detector.features(in: scaled).compactMap { ($0 as? CIQRCodeFeature)?.messageString }
        #expect(decoded == [link])
    }
}

private final class FakePairingApi: PairingApiProtocol, @unchecked Sendable {
    struct Calls {
        var starts: [PairingFlow] = []
        var refreshes = 0
        var cancels = 0
    }

    let calls = OSAllocatedUnfairLock(initialState: Calls())
    var startError: Error?
    var accountNameValue: String?

    func start(flow: PairingFlow) throws -> UInt64 {
        if let startError { throw startError }
        return calls.withLock {
            $0.starts.append(flow)
            return UInt64($0.starts.count)
        }
    }

    func refreshChallengePin() -> Bool {
        calls.withLock { $0.refreshes += 1 }
        return true
    }

    func cancel() {
        calls.withLock { $0.cancels += 1 }
    }

    func current() -> PairingUpdate? { nil }

    func accountName() -> String? { accountNameValue }
}
