import Foundation
import PhoneLinkEngine

/// The app's log file, which the engine keeps in `~/Library/Logs/<bundle id>/` with identifiers and
/// personal data already redacted: installed at launch, and saved or cleared from Settings ›
/// Advanced.
public enum LogArchive {
    /// The log's directory.
    public static func directory(bundleID: String) -> URL {
        let library = FileManager.default.urls(for: .libraryDirectory, in: .userDomainMask)[0]
        return library.appendingPathComponent("Logs", isDirectory: true).appendingPathComponent(bundleID, isDirectory: true)
    }

    /// Installs the log, then writes a first line naming the app's version and the system's, so each
    /// launch can be told apart in a saved log.
    public static func install(bundleID: String, bundle: Bundle = .main) throws {
        try installLog(directory: directory(bundleID: bundleID).path)

        let version = bundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?"
        let build = bundle.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "?"
        let system = ProcessInfo.processInfo.operatingSystemVersionString
        AppLog(category: "app").info(
            "app launched: version \(version, privacy: .public) (\(build, privacy: .public)) on macOS \(system, privacy: .public)"
        )
    }

    /// The bytes the log holds.
    public static var size: UInt64 { logSize() }

    /// The name the save panel suggests: `Phone Link Log 2026-10-31 14.05.log`.
    public static func suggestedFileName(at date: Date, timeZone: TimeZone = .current) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = timeZone
        formatter.dateFormat = "yyyy-MM-dd HH.mm"
        return String(localized: "Phone Link Log \(formatter.string(from: date)).log")
    }

    /// Writes the whole log to `destination`, replacing a file there, off the main actor.
    public static func save(to destination: URL) async throws {
        try await Task.detached { try exportLog(destination: destination.path) }.value
    }

    /// Empties the log.
    public static func clear() throws {
        try clearLog()
    }
}
