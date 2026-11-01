import Foundation
import Observation

private let log = AppLog(category: "updates")

/// The app's updates from its GitHub releases: checks once a day while the user keeps automatic checks
/// on (and on demand), offers the newest release of the user's channel above the running version, and
/// downloads, checks and installs it, then relaunches the app.
///
/// An automatic check that finds a release the user has not skipped opens the update window, once per
/// release and launch; a check the user starts always does, to show what it found.
@MainActor
@Observable
public final class UpdateModel {
    public enum Phase: Equatable {
        case idle
        case checking
        case upToDate
        case available(AppRelease)
        /// The release's image downloads; the fraction received.
        case downloading(AppRelease, Double)
        /// The image is checked and its app is replacing this one.
        case installing(AppRelease)
        case failed(String)
    }

    public private(set) var phase: Phase = .idle

    /// The release on offer, also while it downloads and installs.
    public var offered: AppRelease? {
        switch phase {
        case let .available(release), let .downloading(release, _), let .installing(release): release
        case .idle, .checking, .upToDate, .failed: nil
        }
    }
    public private(set) var lastChecked: Date?
    /// The running app's version.
    public let current: AppVersion
    /// Whether this build has a repository to update from.
    public var isEnabled: Bool { feed != nil }
    public var installability: Installability { installer.installability() }

    /// Check once a day (default on).
    public var checksAutomatically: Bool {
        didSet { defaults.set(checksAutomatically, forKey: PreferenceKeys.updatesCheckAutomatically) }
    }

    /// Offer pre-releases too (default off).
    public var channel: UpdateChannel {
        didSet { defaults.set(channel.rawValue, forKey: PreferenceKeys.updatesChannel) }
    }

    /// The release whose version the user chose to skip: automatic checks do not open the window for it.
    public private(set) var skipped: AppVersion?

    /// Opens the update window.
    @ObservationIgnored public var presentWindow: (() -> Void)?
    /// Quits the app once the update is installed.
    @ObservationIgnored public var terminate: (() -> Void)?

    @ObservationIgnored private let feed: (any ReleaseFeed)?
    @ObservationIgnored private let installer: any UpdateInstalling
    @ObservationIgnored private let defaults: UserDefaults
    @ObservationIgnored private let now: () -> Date
    @ObservationIgnored private var schedule: Task<Void, Never>?
    @ObservationIgnored private var work: Task<Void, Never>?
    /// The release the window was opened for by an automatic check in this launch.
    @ObservationIgnored private var announced: AppVersion?

    /// How long after the last check the next automatic one runs.
    static let interval: TimeInterval = 24 * 60 * 60
    /// How long after launch the first automatic check may run, so it does not compete with connecting
    /// to the phone.
    static let launchDelay: Duration = .seconds(30)
    /// How often the schedule looks whether a check is due.
    static let tick: Duration = .seconds(60 * 60)

    /// The updater of the running app, from the repository its Info.plist names
    /// (`PhoneLinkUpdateRepository`); without one it is off.
    public convenience init(bundle: Bundle = .main) {
        guard let text = bundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String,
              let current = AppVersion(text)
        else {
            preconditionFailure("the app's CFBundleShortVersionString is a Semantic Version")
        }

        let repository = (bundle.object(forInfoDictionaryKey: "PhoneLinkUpdateRepository") as? String) ?? ""
        self.init(
            current: current,
            feed: repository.isEmpty ? nil : GitHubReleaseFeed(repository: repository),
            installer: UpdateInstaller(bundle: bundle),
            defaults: .standard,
            now: Date.init
        )
    }

    init(
        current: AppVersion,
        feed: (any ReleaseFeed)?,
        installer: any UpdateInstalling,
        defaults: UserDefaults,
        now: @escaping () -> Date
    ) {
        self.current = current
        self.feed = feed
        self.installer = installer
        self.defaults = defaults
        self.now = now

        checksAutomatically = defaults.object(forKey: PreferenceKeys.updatesCheckAutomatically) as? Bool ?? true
        channel = defaults.string(forKey: PreferenceKeys.updatesChannel).flatMap { UpdateChannel(rawValue: $0) } ?? .stable
        lastChecked = defaults.object(forKey: PreferenceKeys.updatesLastChecked) as? Date
        skipped = defaults.string(forKey: PreferenceKeys.updatesSkippedVersion).flatMap { AppVersion($0) }
    }

    /// Starts the daily schedule.
    public func start() {
        guard isEnabled, schedule == nil else { return }
        schedule = Task { [weak self] in
            try? await Task.sleep(for: Self.launchDelay)

            while !Task.isCancelled {
                guard let self else { return }
                if self.isDue {
                    let check = Task { await self.check(userInitiated: false) }
                    self.work = check
                    await check.value
                    self.work = nil
                }
                try? await Task.sleep(for: Self.tick)
            }
        }
    }

    /// An automatic check is on, nothing else is under way, and the last check is a day old.
    var isDue: Bool {
        guard checksAutomatically, work == nil else { return false }
        switch phase {
        case .downloading, .installing, .checking: return false
        default: break
        }
        guard let lastChecked else { return true }
        return now().timeIntervalSince(lastChecked) >= Self.interval
    }

    /// Checks now and shows the result in the update window.
    public func checkNow() {
        guard isEnabled, work == nil else {
            presentWindow?()
            return
        }

        presentWindow?()
        work = Task { [weak self] in
            await self?.check(userInitiated: true)
            self?.work = nil
        }
    }

    func check(userInitiated: Bool) async {
        guard let feed else { return }
        phase = .checking
        do {
            let releases = try await feed.releases()

            lastChecked = now()
            defaults.set(lastChecked, forKey: PreferenceKeys.updatesLastChecked)
            guard let release = channel.newest(of: releases, above: current) else {
                log.info("no update above \(current.description, privacy: .public) on \(channel.rawValue, privacy: .public)")
                phase = .upToDate
                return
            }

            log.info("update \(release.version.description, privacy: .public) available")
            phase = .available(release)
            if !userInitiated, release.version != skipped, release.version != announced {
                announced = release.version
                presentWindow?()
            }
        } catch {
            log.error("update check failed: \(error: error)")
            phase = .failed(Self.describe(error))
        }
    }

    /// Downloads, checks and installs the offered release, then quits for the relaunch.
    public func install() {
        guard case let .available(release) = phase, work == nil else { return }
        phase = .downloading(release, 0)

        let installer = installer
        let progress: @Sendable (Double) -> Void = { [weak self] fraction in
            Task { @MainActor [weak self] in
                guard let self, case .downloading = self.phase else { return }
                self.phase = .downloading(release, fraction)
            }
        }

        work = Task { [weak self] in
            defer { self?.work = nil }
            do {
                let staged = try await installer.stage(release, progress: progress)
                self?.phase = .installing(release)
                try installer.install(staged)
                self?.terminate?()
            } catch let error as URLError where error.code == .cancelled {
                self?.phase = .available(release)
            } catch {
                log.error("update \(release.version.description, privacy: .public) not installed: \(error: error)")
                self?.phase = .failed(Self.describe(error))
            }
        }
    }

    /// Opens the update window on what it shows now.
    public func showWindow() {
        presentWindow?()
    }

    /// Stops a download under way.
    public func cancel() {
        work?.cancel()
    }

    /// Automatic checks stop offering this release; a later one is offered again.
    public func skip() {
        guard case let .available(release) = phase else { return }
        skipped = release.version
        defaults.set(release.version.description, forKey: PreferenceKeys.updatesSkippedVersion)
    }

    static func describe(_ error: any Error) -> String {
        switch error {
        case let error as ReleaseFeedError:
            switch error {
            case .rateLimited:
                return String(localized: "GitHub is limiting requests from this network. Try again in an hour.")
            case .http, .malformed:
                return String(localized: "GitHub's list of releases couldn't be read.")
            }
        case let error as UpdateInstallError:
            switch error {
            case .download:
                return String(localized: "The update couldn't be downloaded.")
            case .checksumMismatch, .imageLayout, .wrongApp, .signature:
                return String(localized: "The downloaded update didn't pass Phone Link's checks, so it wasn't installed.")
            case .tool:
                return String(localized: "The update couldn't be prepared for installing.")
            case let .notReplaceable(folder):
                return String(localized: "Phone Link can't replace itself in \(folder). Download the update from its release page instead.")
            }
        case let error as URLError where error.code == .notConnectedToInternet:
            return String(localized: "This Mac isn't connected to the internet.")
        default:
            return error.localizedDescription
        }
    }
}
