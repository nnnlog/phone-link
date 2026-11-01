import AppKit
import Foundation
import IOBluetooth
import PhoneLinkEngine

/// Bluetooth Classic with the phone, for every feature that reaches it over Bluetooth (instant
/// hotspot's side channel today): the radio, the phone among the paired devices, discovery and
/// pairing — which the engine runs when the user starts such a feature and no paired device is the
/// phone — and RFCOMM channels to a service on the phone. IOBluetooth delivers its callbacks on the
/// main run loop, so every IOBluetooth call runs on the main thread; the engine calls this adapter on
/// its own blocking threads, which wait for the main thread's answer.
public final class PhoneBluetoothAdapter: PhoneBluetooth, @unchecked Sendable {
    private let lock = NSLock()
    private var channels: [UInt64: RfcommChannelBox] = [:]
    private var nextHandle: UInt64 = 1

    public init() {}

    public func radioOn() -> Bool {
        onMain {
            guard let controller = IOBluetoothHostController.default() else { return false }
            return controller.powerState == kBluetoothHCIPowerStateON
        }
    }

    public func pairedPhone(names: [String]) -> String? {
        onMain {
            let paired = IOBluetoothDevice.pairedDevices() as? [IOBluetoothDevice] ?? []
            return paired.first { BluetoothProximity.isPhone(deviceName: $0.name, phoneNames: names) }?.addressString
        }
    }

    public func discoverAndPair(names: [String], timeoutMs: UInt64) throws -> String {
        let session = PairingSession(names: names)
        DispatchQueue.main.async { session.start(timeoutMs: timeoutMs) }

        // The inquiry is bounded by `timeoutMs`; the user then confirms the code on both devices.
        let confirmationWait: UInt64 = 120_000
        guard session.done.wait(timeout: .now() + .milliseconds(Int(timeoutMs + confirmationWait))) == .success else {
            DispatchQueue.main.async { session.cancel() }
            throw PairingError.Failed(reason: "pairing did not finish")
        }
        return try session.result.get()
    }

    public func rfcommOpen(address: String, serviceUuid: String, timeoutMs: UInt64) throws -> UInt64 {
        guard let uuid = UUID(uuidString: serviceUuid) else {
            throw RfcommError.Unexpected(message: "not a UUID: \(serviceUuid)")
        }

        let opener = RfcommOpener(address: address, uuid: uuid)
        DispatchQueue.main.async { opener.start() }
        guard opener.done.wait(timeout: .now() + .milliseconds(Int(timeoutMs))) == .success else {
            DispatchQueue.main.async { opener.cancel() }
            throw RfcommError.TimedOut
        }

        let box = try opener.result.get()
        lock.lock()
        defer { lock.unlock() }
        let handle = nextHandle
        nextHandle += 1
        channels[handle] = box
        return handle
    }

    public func refreshServices(address: String, serviceUuid: String, timeoutMs: UInt64) throws -> Bool {
        guard let uuid = UUID(uuidString: serviceUuid) else {
            throw RfcommError.Unexpected(message: "not a UUID: \(serviceUuid)")
        }

        let refresher = ServiceRefresher(address: address, uuid: uuid)
        DispatchQueue.main.async { refresher.start(timeoutMs: timeoutMs) }

        // The refresher ends itself at its deadline; the margin covers a connection still opening.
        let margin: UInt64 = 10_000
        guard refresher.done.wait(timeout: .now() + .milliseconds(Int(timeoutMs + margin))) == .success else {
            DispatchQueue.main.async { refresher.cancel() }
            throw RfcommError.TimedOut
        }
        return try refresher.result.get()
    }

    public func rfcommWrite(channel: UInt64, bytes: Data) throws {
        let box = try channelBox(channel)
        let status: IOReturn = onMain { box.write(bytes) }
        guard status == kIOReturnSuccess else {
            throw RfcommError.Io(reason: String(format: "write failed (0x%08x)", status))
        }
    }

    public func rfcommRead(channel: UInt64, timeoutMs: UInt64) throws -> Data {
        try channelBox(channel).read(timeoutMs: timeoutMs)
    }

    public func rfcommClose(channel: UInt64) {
        lock.lock()
        let box = channels.removeValue(forKey: channel)
        lock.unlock()
        guard let box else { return }
        DispatchQueue.main.async { box.close() }
    }

    private func channelBox(_ handle: UInt64) throws -> RfcommChannelBox {
        lock.lock()
        defer { lock.unlock() }
        guard let box = channels[handle] else {
            throw RfcommError.Io(reason: "the channel is closed")
        }
        return box
    }
}

/// Runs `work` on the main thread and returns its result.
private func onMain<T>(_ work: () -> T) -> T {
    if Thread.isMainThread { return work() }
    return DispatchQueue.main.sync(execute: work)
}

// MARK: - Discovery and pairing

/// One inquiry for the phone and the pairing with the first device that matches. Main thread only,
/// except `done` and `result`, which the waiting thread reads after `done` is signalled.
private final class PairingSession: NSObject, IOBluetoothDeviceInquiryDelegate, @unchecked Sendable {
    let names: [String]
    let done = DispatchSemaphore(value: 0)
    private(set) var result: Result<String, PairingError> = .failure(.NotFound)
    private var inquiry: IOBluetoothDeviceInquiry?
    private var pairing: IOBluetoothDevicePair?
    private var finished = false
    private var found = false

    init(names: [String]) {
        self.names = names
    }

    func start(timeoutMs: UInt64) {
        guard let inquiry = IOBluetoothDeviceInquiry(delegate: self) else {
            finish(.failure(.Failed(reason: "Bluetooth discovery is unavailable")))
            return
        }

        self.inquiry = inquiry
        inquiry.updateNewDeviceNames = true
        inquiry.inquiryLength = UInt8(clamping: max(1, timeoutMs / 1000))

        let status = inquiry.start()
        if status != kIOReturnSuccess {
            finish(.failure(.Failed(reason: String(format: "discovery did not start (0x%08x)", status))))
        }
    }

    func cancel() {
        inquiry?.stop()
        pairing?.stop()
        finish(.failure(.Failed(reason: "pairing did not finish")))
    }

    private func finish(_ outcome: Result<String, PairingError>) {
        guard !finished else { return }
        finished = true
        result = outcome
        done.signal()
    }

    private func consider(_ device: IOBluetoothDevice) {
        guard !found, BluetoothProximity.isPhone(deviceName: device.name, phoneNames: names) else { return }
        found = true
        inquiry?.stop()

        guard let pair = IOBluetoothDevicePair(device: device) else {
            finish(.failure(.Failed(reason: "pairing is unavailable")))
            return
        }
        pairing = pair
        pair.delegate = self
        let status = pair.start()
        if status != kIOReturnSuccess {
            finish(.failure(.Failed(reason: String(format: "pairing did not start (0x%08x)", status))))
        }
    }

    func deviceInquiryDeviceFound(_ sender: IOBluetoothDeviceInquiry!, device: IOBluetoothDevice!) {
        if let device { consider(device) }
    }

    func deviceInquiryDeviceNameUpdated(_ sender: IOBluetoothDeviceInquiry!, device: IOBluetoothDevice!, devicesRemaining: UInt32) {
        if let device { consider(device) }
    }

    func deviceInquiryComplete(_ sender: IOBluetoothDeviceInquiry!, error: IOReturn, aborted: Bool) {
        if !found { finish(.failure(.NotFound)) }
    }

    // IOBluetoothDevicePairDelegate (an informal protocol).

    @objc func devicePairingUserConfirmationRequest(_ sender: Any!, numericValue: BluetoothNumericValue) {
        // IOBluetooth calls its delegates on the main run loop.
        let confirmed = MainActor.assumeIsolated {
            NSApp.activate()
            let alert = NSAlert()
            let code = String(format: "%06u", numericValue)
            alert.messageText = String(localized: "Pair with Your Phone?")
            alert.informativeText = String(localized: "Make sure your phone shows the code \(code), then confirm on both devices.")
            alert.addButton(withTitle: String(localized: "Pair", comment: "Button that pairs the phone over Bluetooth"))
            alert.addButton(withTitle: String(localized: "Cancel"))
            return alert.runModal() == .alertFirstButtonReturn
        }

        guard let pair = sender as? IOBluetoothDevicePair else {
            finish(.failure(.Failed(reason: "the pairing request came from \(String(describing: sender))")))
            return
        }

        do {
            try CoreBluetoothPairingReply.reply(to: pair, accept: confirmed, passkey: numericValue)
        } catch {
            pair.stop()
            finish(.failure(error as? PairingError ?? .Failed(reason: String(describing: error))))
        }
    }

    @objc func devicePairingFinished(_ sender: Any!, error: IOReturn) {
        if error == kIOReturnSuccess, let address = pairing?.device()?.addressString {
            finish(.success(address))
        } else {
            finish(.failure(.Failed(reason: String(format: "pairing failed (0x%08x)", error))))
        }
    }
}

/// Answers a pending pairing request through CoreBluetooth's pairing agent. On current macOS
/// `IOBluetoothDevicePair.replyUserConfirmation(_:)` does nothing (IOBluetooth logs "…no-op. Should
/// be done at CB Layer"), so the phone's numeric comparison times out. IOBluetooth's CoreBluetooth
/// coordinator owns the classic manager whose pairing agent received the request; the answer goes to
/// that agent. Its `data` argument is a dictionary merged into the message to bluetoothd, which refuses
/// to accept a numeric comparison without the request's passkey under `kCBMsgArgPairingPasskey`.
/// None of these classes or selectors is public, so a missing one is an error.
private enum CoreBluetoothPairingReply {
    private typealias PairingType = @convention(c) (AnyObject, Selector) -> Int
    private typealias Respond = @convention(c) (AnyObject, Selector, AnyObject, Int, Bool, AnyObject?) -> Void

    /// Main thread (IOBluetooth's pairing callbacks run there).
    static func reply(to pair: IOBluetoothDevicePair, accept: Bool, passkey: BluetoothNumericValue) throws {
        guard let coordinatorClass = NSClassFromString("IOBluetoothCoreBluetoothCoordinator") as? NSObject.Type else {
            throw PairingError.Failed(reason: "IOBluetoothCoreBluetoothCoordinator is missing")
        }

        let coordinator = try object(coordinatorClass, "sharedInstance")
        let manager = try object(coordinator, "classicManager")
        let agent = try object(manager, "sharedPairingAgent")

        guard let address = pair.device()?.addressString else {
            throw PairingError.Failed(reason: "the pairing has no device")
        }
        let lookup = NSSelectorFromString("peerForAddressString:")
        guard coordinator.responds(to: lookup) else {
            throw PairingError.Failed(reason: "peerForAddressString: is missing")
        }

        // IOBluetooth writes `78-33-c6-…`; CoreBluetooth's peers carry `78:33:C6:…`.
        let candidates = [address, address.replacingOccurrences(of: "-", with: ":").uppercased()]
        guard let peer = candidates.lazy.compactMap({ coordinator.perform(lookup, with: $0)?.takeUnretainedValue() }).first
        else {
            throw PairingError.Failed(reason: "CoreBluetooth knows no peer \(address)")
        }

        let typeSelector = NSSelectorFromString("currentPairingType")
        let respondSelector = NSSelectorFromString("respondToPairingRequest:type:accept:data:")
        guard pair.responds(to: typeSelector), let respondTarget = agent as? NSObject,
              respondTarget.responds(to: respondSelector)
        else {
            throw PairingError.Failed(reason: "the pairing agent cannot answer")
        }

        let type = unsafeBitCast(pair.method(for: typeSelector), to: PairingType.self)(pair, typeSelector)
        let respond = unsafeBitCast(respondTarget.method(for: respondSelector), to: Respond.self)
        let data: NSDictionary = ["kCBMsgArgPairingPasskey": NSNumber(value: passkey)]
        respond(respondTarget, respondSelector, peer, type, accept, data)
    }

    private static func object(_ target: AnyObject, _ selector: String) throws -> NSObject {
        let sel = NSSelectorFromString(selector)
        guard target.responds(to: sel), let value = target.perform(sel)?.takeUnretainedValue() as? NSObject else {
            throw PairingError.Failed(reason: "\(selector) gave nothing")
        }
        return value
    }
}

// MARK: - RFCOMM

/// Opens one RFCOMM channel: the service's record among the phone's SDP records, then the channel.
/// Main thread only, except `done` and `result`.
///
/// On current macOS IOBluetooth sends no SDP query to the device: `performSDPQuery(_:uuids:)` returns
/// success and does nothing, so its callback never comes. The records are the ones bluetoothd last
/// read from the phone — when it paired, or when `ServiceRefresher` made it read them again — which
/// `getServiceRecord(for:)` reads directly.
private final class RfcommOpener: NSObject, @unchecked Sendable {
    let address: String
    let uuid: UUID
    let done = DispatchSemaphore(value: 0)
    private(set) var result: Result<RfcommChannelBox, RfcommError> = .failure(.TimedOut)
    private var finished = false

    init(address: String, uuid: UUID) {
        self.address = address
        self.uuid = uuid
    }

    private var sdpUUID: IOBluetoothSDPUUID {
        var bytes = uuid.uuid
        return IOBluetoothSDPUUID(bytes: &bytes, length: 16)
    }

    func start() {
        guard let device = IOBluetoothDevice(addressString: address) else {
            finish(.failure(.Unexpected(message: "not a Bluetooth address: \(address)")))
            return
        }
        guard let record = device.getServiceRecord(for: sdpUUID) else {
            finish(.failure(.ServiceNotFound))
            return
        }

        var channelID: BluetoothRFCOMMChannelID = 0
        guard record.getRFCOMMChannelID(&channelID) == kIOReturnSuccess else {
            finish(.failure(.ServiceNotFound))
            return
        }

        let box = RfcommChannelBox()
        var channel: IOBluetoothRFCOMMChannel?
        let status = device.openRFCOMMChannelSync(&channel, withChannelID: channelID, delegate: box)
        guard status == kIOReturnSuccess, let channel else {
            finish(.failure(.ConnectFailed(reason: String(format: "the channel did not open (0x%08x)", status))))
            return
        }

        box.channel = channel
        finish(.success(box))
    }

    func cancel() {
        finish(.failure(.TimedOut))
    }

    private func finish(_ outcome: Result<RfcommChannelBox, RfcommError>) {
        guard !finished else {
            if case let .success(box) = outcome { box.close() }
            return
        }
        finished = true
        result = outcome
        done.signal()
    }

}

/// Makes bluetoothd read the device's service records again and watches for the service's record.
/// Main thread only, except `done` and `result`.
///
/// With no SDP query available (see `RfcommOpener`), the records change only when bluetoothd reads
/// them itself. On a user's Mac whose records lacked the side channel, connecting to the phone from
/// System Settings › Bluetooth made it reachable, so the refresher assumes a baseband connection is
/// what makes bluetoothd read them again. It opens a baseband connection when there is
/// none, polls `getServiceRecord(for:)` until the record is there or the deadline passes, and closes
/// the connection it opened.
private final class ServiceRefresher: NSObject, @unchecked Sendable {
    let address: String
    let uuid: UUID
    let done = DispatchSemaphore(value: 0)
    private(set) var result: Result<Bool, RfcommError> = .failure(.TimedOut)
    private var finished = false
    private var device: IOBluetoothDevice?
    private var opened = false
    private var timer: Timer?
    private var deadline = Date()

    /// How often the records are looked at while waiting.
    private static let pollInterval: TimeInterval = 0.25

    init(address: String, uuid: UUID) {
        self.address = address
        self.uuid = uuid
    }

    private var sdpUUID: IOBluetoothSDPUUID {
        var bytes = uuid.uuid
        return IOBluetoothSDPUUID(bytes: &bytes, length: 16)
    }

    func start(timeoutMs: UInt64) {
        guard let device = IOBluetoothDevice(addressString: address) else {
            finish(.failure(.Unexpected(message: "not a Bluetooth address: \(address)")))
            return
        }

        self.device = device
        deadline = Date(timeIntervalSinceNow: Double(timeoutMs) / 1000)
        if device.isConnected() {
            watch()
            return
        }

        let status = device.openConnection(self)
        guard status == kIOReturnSuccess else {
            finish(.failure(.ConnectFailed(reason: String(format: "the connection did not open (0x%08x)", status))))
            return
        }
        opened = true
    }

    func cancel() {
        finish(.failure(.TimedOut))
    }

    // The target of `openConnection(_:)` (an informal protocol), called on the main run loop.
    @objc func connectionComplete(_ device: IOBluetoothDevice!, status: IOReturn) {
        guard !finished else { return }
        guard status == kIOReturnSuccess else {
            opened = false
            finish(.failure(.Unreachable))
            return
        }
        watch()
    }

    private func watch() {
        if check() { return }
        timer = Timer.scheduledTimer(withTimeInterval: Self.pollInterval, repeats: true) { [weak self] _ in
            _ = self?.check()
        }
    }

    /// Finishes when the record is there or the deadline has passed; returns whether it finished.
    private func check() -> Bool {
        guard !finished, let device else { return true }

        if device.getServiceRecord(for: sdpUUID) != nil {
            finish(.success(true))
            return true
        }
        if Date() >= deadline {
            finish(.success(false))
            return true
        }
        return false
    }

    private func finish(_ outcome: Result<Bool, RfcommError>) {
        timer?.invalidate()
        timer = nil
        if opened {
            opened = false
            device?.closeConnection()
        }

        guard !finished else { return }
        finished = true
        result = outcome
        done.signal()
    }
}

/// An open channel: writes on the main thread, and the bytes the delegate receives queued for a
/// reader on another thread.
private final class RfcommChannelBox: NSObject, IOBluetoothRFCOMMChannelDelegate, @unchecked Sendable {
    var channel: IOBluetoothRFCOMMChannel?
    private let condition = NSCondition()
    private var received = Data()
    private var closed = false

    /// Main thread.
    func write(_ bytes: Data) -> IOReturn {
        guard let channel else { return kIOReturnNotOpen }
        let mtu = max(1, Int(channel.getMTU()))
        var data = bytes
        var offset = 0

        while offset < data.count {
            let length = min(mtu, data.count - offset)
            let status = data.withUnsafeMutableBytes { raw in
                channel.writeSync(raw.baseAddress!.advanced(by: offset), length: UInt16(length))
            }
            if status != kIOReturnSuccess { return status }
            offset += length
        }

        data.removeAll()
        return kIOReturnSuccess
    }

    func read(timeoutMs: UInt64) throws -> Data {
        let deadline = Date(timeIntervalSinceNow: Double(timeoutMs) / 1000)
        condition.lock()
        defer { condition.unlock() }
        while received.isEmpty && !closed {
            if !condition.wait(until: deadline) {
                throw RfcommError.TimedOut
            }
        }

        let bytes = received
        received.removeAll()
        return bytes
    }

    /// Main thread.
    func close() {
        channel?.close()
        channel = nil
        markClosed()
    }

    private func markClosed() {
        condition.lock()
        closed = true
        condition.broadcast()
        condition.unlock()
    }

    func rfcommChannelData(_ rfcommChannel: IOBluetoothRFCOMMChannel!, data dataPointer: UnsafeMutableRawPointer!, length dataLength: Int) {
        guard let dataPointer else { return }
        condition.lock()
        received.append(dataPointer.assumingMemoryBound(to: UInt8.self), count: dataLength)
        condition.broadcast()
        condition.unlock()
    }

    func rfcommChannelClosed(_ rfcommChannel: IOBluetoothRFCOMMChannel!) {
        markClosed()
    }
}
