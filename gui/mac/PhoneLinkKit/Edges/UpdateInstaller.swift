import Foundation
import Security
import CryptoKit

private let log = AppLog(category: "updates")

/// Whether this copy of the app can replace itself with a release.
public enum Installability: Equatable, Sendable {
    case available
    /// This copy has no team signature (an ad-hoc build), so a download cannot be checked against it.
    case unsigned
    /// macOS runs this copy from a read-only place (App Translocation, a disk image), or its folder is
    /// not writable for this user.
    case notReplaceable(folder: String)
}

/// A release's app, downloaded and checked, ready to take the running app's place.
public struct StagedUpdate: Equatable, Sendable {
    public let app: URL
    public let version: AppVersion
}

public enum UpdateInstallError: Error, Equatable {
    case download(status: Int)
    /// The downloaded image's SHA-256 is not the one GitHub recorded for it.
    case checksumMismatch
    /// The image holds no app, or more than one.
    case imageLayout
    /// The app in the image is not this app at the release's version and a higher build.
    case wrongApp(String)
    /// The app in the image is not signed by this app's team with Developer ID, or Gatekeeper rejects it.
    case signature(String)
    /// A command-line tool the installer runs failed.
    case tool(String, status: Int32)
    case notReplaceable(folder: String)
}

/// Downloads, checks and installs releases.
public protocol UpdateInstalling: Sendable {
    func installability() -> Installability
    /// Downloads the release's image, checks its digest, copies its app next to the running app and
    /// checks that copy's identity, version and signature. `progress` gets the downloaded fraction.
    func stage(_ release: AppRelease, progress: @escaping @Sendable (Double) -> Void) async throws -> StagedUpdate
    /// Puts the staged app in the running app's place, and starts a process that opens it once this
    /// process has exited. The caller quits the app next.
    func install(_ staged: StagedUpdate) throws
}

/// Installs a release over the running app bundle, the way dragging it to Applications would, after
/// checking what it installs: GitHub's SHA-256 of the image, then that the app inside is this app (its
/// bundle identifier), at the release's version and a higher build, signed with Developer ID by the team
/// that signed the running app, and accepted by Gatekeeper.
public struct UpdateInstaller: UpdateInstalling {
    private let bundle: URL
    private let bundleID: String
    private let build: Int
    private let session: URLSession

    public init(bundle: Bundle = .main, session: URLSession = .shared) {
        guard let bundleID = bundle.bundleIdentifier,
              let build = (bundle.object(forInfoDictionaryKey: "CFBundleVersion") as? String).flatMap(Int.init)
        else {
            preconditionFailure("the app's Info.plist has a bundle identifier and a whole-number CFBundleVersion")
        }

        self.bundle = bundle.bundleURL
        self.bundleID = bundleID
        self.build = build
        self.session = session
    }

    public func installability() -> Installability {
        guard Self.teamIdentifier() != nil else { return .unsigned }

        let folder = bundle.deletingLastPathComponent()
        let files = FileManager.default
        if bundle.path.contains("/AppTranslocation/")
            || !files.isWritableFile(atPath: folder.path)
            || !files.isWritableFile(atPath: bundle.path) {
            return .notReplaceable(folder: folder.path)
        }
        return .available
    }

    public func stage(_ release: AppRelease, progress: @escaping @Sendable (Double) -> Void) async throws -> StagedUpdate {
        guard let team = Self.teamIdentifier() else {
            throw UpdateInstallError.signature("this copy of Phone Link has no team signature")
        }

        let work = try workDirectory()
        defer { try? FileManager.default.removeItem(at: work) }

        let image = work.appendingPathComponent(release.image.name)
        try await download(release.image.url, to: image, size: release.image.size, progress: progress)
        guard try Self.sha256(of: image) == release.image.sha256 else {
            throw UpdateInstallError.checksumMismatch
        }

        let mount = work.appendingPathComponent("mount", isDirectory: true)
        try FileManager.default.createDirectory(at: mount, withIntermediateDirectories: false)
        try await Self.run("/usr/bin/hdiutil", ["attach", "-quiet", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint", mount.path, image.path])

        let staged: URL
        do {
            let apps = try FileManager.default.contentsOfDirectory(at: mount, includingPropertiesForKeys: nil)
                .filter { $0.pathExtension == "app" }
            guard apps.count == 1 else { throw UpdateInstallError.imageLayout }

            // Beside the running app, on its volume, so installing is a rename.
            let staging = try FileManager.default.url(
                for: .itemReplacementDirectory, in: .userDomainMask, appropriateFor: bundle, create: true
            )
            staged = staging.appendingPathComponent(bundle.lastPathComponent, isDirectory: true)
            try await Self.run("/usr/bin/ditto", [apps[0].path, staged.path])
        } catch {
            await Self.detach(mount)
            throw error
        }
        await Self.detach(mount)

        do {
            try Self.checkInfo(
                of: staged, bundleID: bundleID, version: release.version, aboveBuild: build
            )
            try Self.checkSignature(of: staged, bundleID: bundleID, team: team)
            try await Self.run("/usr/sbin/spctl", ["--assess", "--type", "execute", staged.path]) { status in
                UpdateInstallError.signature("Gatekeeper rejects it (spctl status \(status))")
            }
        } catch {
            try? FileManager.default.removeItem(at: staged.deletingLastPathComponent())
            throw error
        }

        log.info("update \(release.version.description, privacy: .public) staged")
        return StagedUpdate(app: staged, version: release.version)
    }

    public func install(_ staged: StagedUpdate) throws {
        if case let .notReplaceable(folder) = installability() {
            throw UpdateInstallError.notReplaceable(folder: folder)
        }

        _ = try FileManager.default.replaceItemAt(bundle, withItemAt: staged.app)
        try? FileManager.default.removeItem(at: staged.app.deletingLastPathComponent())
        log.info("update \(staged.version.description, privacy: .public) installed; relaunching")
        try Self.relaunchAfterExit(bundle)
    }

    // MARK: - Steps

    /// An empty folder for the download, in the app's caches.
    private func workDirectory() throws -> URL {
        let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        let work = caches.appendingPathComponent(bundleID, isDirectory: true)
            .appendingPathComponent("Update", isDirectory: true)
        try? FileManager.default.removeItem(at: work)
        try FileManager.default.createDirectory(at: work, withIntermediateDirectories: true)
        return work
    }

    /// Downloads `url` to `destination`, reporting the fraction of `size` received every tenth of a second.
    private func download(_ url: URL, to destination: URL, size: Int64, progress: @escaping @Sendable (Double) -> Void) async throws {
        let box = DownloadTaskBox()
        let poll = Task {
            while !Task.isCancelled {
                if let task = box.task, size > 0 {
                    progress(min(1, Double(task.countOfBytesReceived) / Double(size)))
                }
                try? await Task.sleep(for: .milliseconds(100))
            }
        }
        defer { poll.cancel() }

        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, any Error>) in
                let task = session.downloadTask(with: url) { file, response, error in
                    // The system deletes `file` when this handler returns, so it moves here.
                    if let error {
                        continuation.resume(throwing: error)
                        return
                    }

                    let status = (response as? HTTPURLResponse)?.statusCode ?? 0
                    guard status == 200, let file else {
                        continuation.resume(throwing: UpdateInstallError.download(status: status))
                        return
                    }

                    do {
                        try FileManager.default.moveItem(at: file, to: destination)
                        continuation.resume()
                    } catch {
                        continuation.resume(throwing: error)
                    }
                }
                box.task = task
                task.resume()
            }
        } onCancel: {
            box.task?.cancel()
        }

        progress(1)
    }

    static func sha256(of file: URL) throws -> Data {
        let handle = try FileHandle(forReadingFrom: file)
        defer { try? handle.close() }
        var hash = SHA256()
        while let chunk = try handle.read(upToCount: 1 << 20), !chunk.isEmpty {
            hash.update(data: chunk)
        }
        return Data(hash.finalize())
    }

    /// The app at `app` is this app (`bundleID`), at `version`, with a build above `aboveBuild`.
    static func checkInfo(of app: URL, bundleID: String, version: AppVersion, aboveBuild: Int) throws {
        guard let info = NSDictionary(contentsOf: app.appendingPathComponent("Contents/Info.plist")) else {
            throw UpdateInstallError.wrongApp("no Info.plist")
        }
        guard info["CFBundleIdentifier"] as? String == bundleID else {
            throw UpdateInstallError.wrongApp("bundle identifier \(info["CFBundleIdentifier"] ?? "none")")
        }
        guard info["CFBundleShortVersionString"] as? String == version.description else {
            throw UpdateInstallError.wrongApp("version \(info["CFBundleShortVersionString"] ?? "none"), not \(version)")
        }
        guard let build = (info["CFBundleVersion"] as? String).flatMap(Int.init), build > aboveBuild else {
            throw UpdateInstallError.wrongApp("build \(info["CFBundleVersion"] ?? "none"), not above \(aboveBuild)")
        }
    }

    /// The code requirement a release must meet: this bundle identifier, signed with a Developer ID
    /// Application certificate (Apple's Developer ID intermediate, the leaf's Developer ID extension) of
    /// `team`.
    static func requirement(bundleID: String, team: String) -> String {
        """
        identifier "\(bundleID)" and anchor apple generic \
        and certificate 1[field.1.2.840.113635.100.6.2.6] \
        and certificate leaf[field.1.2.840.113635.100.6.1.13] \
        and certificate leaf[subject.OU] = "\(team)"
        """
    }

    private static func checkSignature(of app: URL, bundleID: String, team: String) throws {
        var code: SecStaticCode?
        var requirement: SecRequirement?
        guard SecStaticCodeCreateWithPath(app as CFURL, [], &code) == errSecSuccess, let code,
              SecRequirementCreateWithString(Self.requirement(bundleID: bundleID, team: team) as CFString, [], &requirement) == errSecSuccess,
              let requirement
        else {
            throw UpdateInstallError.signature("its signature could not be read")
        }

        let flags = SecCSFlags(rawValue: kSecCSCheckAllArchitectures | kSecCSStrictValidate | kSecCSCheckNestedCode)
        var error: Unmanaged<CFError>?
        let status = SecStaticCodeCheckValidityWithErrors(code, flags, requirement, &error)
        guard status == errSecSuccess else {
            let reason = error.map { CFErrorCopyDescription($0.takeRetainedValue()) as String } ?? "status \(status)"
            throw UpdateInstallError.signature(reason)
        }
    }

    /// The team that signed the running app; `nil` for an ad-hoc signature.
    static func teamIdentifier() -> String? {
        var code: SecCode?
        var staticCode: SecStaticCode?
        var info: CFDictionary?
        guard SecCodeCopySelf([], &code) == errSecSuccess, let code,
              SecCodeCopyStaticCode(code, [], &staticCode) == errSecSuccess, let staticCode,
              SecCodeCopySigningInformation(staticCode, SecCSFlags(rawValue: kSecCSSigningInformation), &info) == errSecSuccess
        else { return nil }
        return (info as? [String: Any])?[kSecCodeInfoTeamIdentifier as String] as? String
    }

    /// A shell in a session of its own (so it outlives this process) that waits for this process to
    /// exit, then opens `app`. If it never runs, the update is installed all the same and opens with the
    /// app's next launch.
    private static func relaunchAfterExit(_ app: URL) throws {
        let script = "while /bin/kill -0 \"$1\" 2>/dev/null; do /bin/sleep 0.2; done; exec /usr/bin/open \"$2\""
        let args = ["/bin/sh", "-c", script, "sh", String(getpid()), app.path]

        var attributes: posix_spawnattr_t?
        posix_spawnattr_init(&attributes)
        defer { posix_spawnattr_destroy(&attributes) }
        posix_spawnattr_setflags(&attributes, Int16(POSIX_SPAWN_SETSID))
        var cArgs = args.map { strdup($0) } + [nil]
        defer { cArgs.forEach { free($0) } }

        var pid: pid_t = 0
        let status = posix_spawn(&pid, "/bin/sh", nil, &attributes, &cArgs, environ)
        guard status == 0 else {
            throw UpdateInstallError.tool("sh", status: status)
        }
    }

    private static func detach(_ mount: URL) async {
        do {
            try await run("/usr/bin/hdiutil", ["detach", "-quiet", mount.path])
        } catch {
            log.notice("update image not detached: \(error: error); forcing it")
            try? await run("/usr/bin/hdiutil", ["detach", "-quiet", "-force", mount.path])
        }
    }

    /// Runs a tool and waits for it; a nonzero exit throws `failure(status)`, by default `.tool`.
    private static func run(
        _ tool: String,
        _ arguments: [String],
        failure: (@Sendable (Int32) -> UpdateInstallError)? = nil
    ) async throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: tool)
        process.arguments = arguments
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice

        let status: Int32 = try await withCheckedThrowingContinuation { continuation in
            process.terminationHandler = { continuation.resume(returning: $0.terminationStatus) }
            do {
                try process.run()
            } catch {
                process.terminationHandler = nil
                continuation.resume(throwing: error)
            }
        }

        guard status == 0 else {
            let name = URL(fileURLWithPath: tool).lastPathComponent
            throw failure?(status) ?? UpdateInstallError.tool(name, status: status)
        }
    }
}

/// The download task, shared with the progress poll and the cancellation handler.
private final class DownloadTaskBox: @unchecked Sendable {
    private let lock = NSLock()
    private var _task: URLSessionDownloadTask?

    var task: URLSessionDownloadTask? {
        get { lock.withLock { _task } }
        set { lock.withLock { _task = newValue } }
    }
}
