import Foundation
import Testing
@testable import PhoneLinkKit

@Suite struct AppVersionTests {
    @Test func versionsParseOnlyInTheirExactForm() {
        #expect(AppVersion("1.2.3")?.description == "1.2.3")
        #expect(AppVersion("0.2.0-beta.1")?.preRelease == ["beta", "1"])
        #expect(AppVersion(tag: "v0.2.0-rc.10")?.description == "0.2.0-rc.10")

        for text in ["1.2", "1.2.3.4", "01.2.3", "1.2.3-", "1.2.3-beta..1", "1.2.3-beta.01", "1.2.3+build", "v1.2.3", " 1.2.3", "1.2.٣"] {
            #expect(AppVersion(text) == nil, "\(text)")
        }
        #expect(AppVersion(tag: "1.2.3") == nil, "a tag starts with v")
    }

    @Test func versionsFollowSemanticVersioningPrecedence() throws {
        let ordered = ["0.9.9", "1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta", "1.0.0-beta.2",
                       "1.0.0-beta.11", "1.0.0-rc.1", "1.0.0", "1.0.1", "1.1.0", "2.0.0"]
        let versions = try ordered.map { try #require(AppVersion($0)) }

        for (lower, higher) in zip(versions, versions.dropFirst()) {
            #expect(lower < higher, "\(lower) < \(higher)")
            #expect(!(higher < lower))
        }

        #expect(versions.shuffled().sorted() == versions)
    }
}

@Suite struct ReleaseFeedTests {
    static let digest = String(repeating: "ab", count: 32)

    static func release(_ tag: String, draft: Bool = false, assets: String? = nil) -> String {
        let version = tag.dropFirst()
        let assets = assets ?? """
            [{"name": "PhoneLink-\(version).dmg", "size": 1000,
              "browser_download_url": "https://github.com/o/r/releases/download/\(tag)/PhoneLink-\(version).dmg",
              "digest": "sha256:\(digest)"}]
            """

        return """
            {"tag_name": "\(tag)", "body": "Notes for \(tag)", "draft": \(draft), "prerelease": false,
             "html_url": "https://github.com/o/r/releases/tag/\(tag)", "published_at": "2026-10-01T12:00:00Z",
             "assets": \(assets)}
            """
    }

    static func list(_ releases: String...) -> Data {
        Data("[\(releases.joined(separator: ","))]".utf8)
    }

    @Test func aReleaseIsATaggedVersionWithItsImage() throws {
        let releases = try GitHubReleaseFeed.parse(Self.list(
            Self.release("v0.3.0", assets: "[]"),
            Self.release("v0.2.0-beta.1"),
            Self.release("nightly"),
            Self.release("v0.1.1", assets: #"[{"name": "Other.zip", "size": 1, "browser_download_url": "https://x/y", "digest": null}]"#),
            Self.release("v0.1.0")
        ))

        #expect(releases.map(\.version.description) == ["0.2.0-beta.1", "0.1.0"])
        let beta = releases[0]
        #expect(beta.notes == "Notes for v0.2.0-beta.1")
        #expect(beta.page.absoluteString == "https://github.com/o/r/releases/tag/v0.2.0-beta.1")
        #expect(beta.image.name == "PhoneLink-0.2.0-beta.1.dmg")
        #expect(beta.image.size == 1000)
        #expect(beta.image.sha256 == Data(repeating: 0xAB, count: 32))
        #expect(beta.published == Date(timeIntervalSince1970: 1_790_856_000))
    }

    @Test func anImageWithoutADigestIsAnError() {
        let image = #"[{"name": "PhoneLink-0.2.0.dmg", "size": 1, "browser_download_url": "https://x/y", "digest": null}]"#
        #expect(throws: ReleaseFeedError.self) {
            try GitHubReleaseFeed.parse(Self.list(Self.release("v0.2.0", assets: image)))
        }

        let short = #"[{"name": "PhoneLink-0.2.0.dmg", "size": 1, "browser_download_url": "https://x/y", "digest": "sha256:abcd"}]"#
        #expect(throws: ReleaseFeedError.self) {
            try GitHubReleaseFeed.parse(Self.list(Self.release("v0.2.0", assets: short)))
        }

        #expect(throws: ReleaseFeedError.self) { try GitHubReleaseFeed.parse(Data("{}".utf8)) }
    }

    @Test func theChannelPicksTheNewestReleaseAboveTheRunningVersion() throws {
        let releases = try GitHubReleaseFeed.parse(Self.list(
            Self.release("v0.3.0-beta.1"), Self.release("v0.2.0"), Self.release("v0.2.1-rc.1"), Self.release("v0.1.0")
        ))

        let current = try #require(AppVersion("0.1.0"))
        #expect(UpdateChannel.stable.newest(of: releases, above: current)?.version.description == "0.2.0")
        #expect(UpdateChannel.beta.newest(of: releases, above: current)?.version.description == "0.3.0-beta.1")

        let newest = try #require(AppVersion("0.3.0"))
        #expect(UpdateChannel.beta.newest(of: releases, above: newest) == nil)
    }

    @Test func aDraftIsNoRelease() throws {
        #expect(try GitHubReleaseFeed.parse(Self.list(Self.release("v0.2.0", draft: true))).isEmpty)
    }
}

@Suite struct UpdateInstallerTests {
    @Test func theCodeRequirementNamesTheAppTheTeamAndDeveloperID() {
        #expect(UpdateInstaller.requirement(bundleID: "dev.example.App", team: "ABCDE12345") == """
            identifier "dev.example.App" and anchor apple generic \
            and certificate 1[field.1.2.840.113635.100.6.2.6] \
            and certificate leaf[field.1.2.840.113635.100.6.1.13] \
            and certificate leaf[subject.OU] = "ABCDE12345"
            """)
    }

    @Test func theStagedAppMustBeThisAppAtTheReleaseVersionAndAHigherBuild() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: folder) }
        let app = folder.appendingPathComponent("PhoneLink.app")
        try FileManager.default.createDirectory(at: app.appendingPathComponent("Contents"), withIntermediateDirectories: true)

        func write(_ id: String, _ version: String, _ build: String) throws {
            let info: NSDictionary = ["CFBundleIdentifier": id, "CFBundleShortVersionString": version, "CFBundleVersion": build]
            try info.write(to: app.appendingPathComponent("Contents/Info.plist"))
        }

        let version = try #require(AppVersion("0.2.0"))
        func check() throws {
            try UpdateInstaller.checkInfo(of: app, bundleID: "dev.example.App", version: version, aboveBuild: 4)
        }

        try write("dev.example.App", "0.2.0", "5")
        try check()
        try write("dev.example.Other", "0.2.0", "5")
        #expect(throws: UpdateInstallError.self) { try check() }
        try write("dev.example.App", "0.2.1", "5")
        #expect(throws: UpdateInstallError.self) { try check() }
        try write("dev.example.App", "0.2.0", "4")
        #expect(throws: UpdateInstallError.self) { try check() }
    }

    @Test func theDigestIsTheFilesSHA256() throws {
        let file = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: file) }
        try Data("abc".utf8).write(to: file)
        #expect(try UpdateInstaller.sha256(of: file) == Data(hex: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"))
    }
}

@MainActor
@Suite struct UpdateModelTests {
    private let feed = ReleaseFeedFake()
    private let installer = UpdateInstallerFake()
    private let defaults = UserDefaults(suiteName: "UpdateModelTests-\(UUID().uuidString)")!
    private let clock = TestClock()

    private func model(current: String = "0.1.0", feed: ReleaseFeedFake?? = .none) -> UpdateModel {
        let clock = clock
        return UpdateModel(
            current: AppVersion(current)!,
            feed: feed ?? self.feed,
            installer: installer,
            defaults: defaults,
            now: { clock.now }
        )
    }

    private static func release(_ version: String) -> AppRelease {
        AppRelease(
            version: AppVersion(version)!,
            notes: "",
            page: URL(string: "https://github.com/o/r/releases/tag/v\(version)")!,
            published: nil,
            image: .init(name: "PhoneLink-\(version).dmg", url: URL(string: "https://x/\(version)")!, size: 1, sha256: Data(count: 32))
        )
    }

    @Test func anAutomaticCheckOpensTheWindowOncePerReleaseAndNotForASkippedOne() async {
        let model = model()
        var opened = 0
        model.presentWindow = { opened += 1 }

        feed.releases = [Self.release("0.2.0")]
        await model.check(userInitiated: false)
        #expect(model.phase == .available(Self.release("0.2.0")))
        #expect(opened == 1)

        await model.check(userInitiated: false)
        #expect(opened == 1, "already announced in this launch")

        model.skip()
        let next = self.model()
        next.presentWindow = { opened += 1 }
        await next.check(userInitiated: false)
        #expect(opened == 1, "skipped, remembered across launches")

        feed.releases = [Self.release("0.2.1"), Self.release("0.2.0")]
        await next.check(userInitiated: false)
        #expect(opened == 2, "a newer release than the skipped one")
    }

    @Test func checksFollowTheChannelAndRecordTheirTime() async {
        let model = model()
        feed.releases = [Self.release("0.2.0-beta.1")]
        await model.check(userInitiated: true)
        #expect(model.phase == .upToDate)
        #expect(model.lastChecked == clock.now)

        model.channel = .beta
        await model.check(userInitiated: true)
        #expect(model.offered?.version.description == "0.2.0-beta.1")
        #expect(self.model().channel == .beta, "kept in the preferences")
    }

    @Test func aCheckIsDueADayAfterTheLastWhileAutomaticChecksAreOn() async {
        let model = model()
        #expect(model.isDue, "never checked")
        await model.check(userInitiated: false)
        #expect(!model.isDue)

        clock.now += UpdateModel.interval - 1
        #expect(!model.isDue)
        clock.now += 1
        #expect(model.isDue)

        model.checksAutomatically = false
        #expect(!model.isDue)
    }

    @Test func aFailedCheckSaysWhy() async {
        let model = model()
        feed.error = ReleaseFeedError.rateLimited
        await model.check(userInitiated: true)

        guard case let .failed(message) = model.phase else {
            Issue.record("phase \(model.phase)")
            return
        }
        #expect(message.contains("GitHub"))
        #expect(model.lastChecked == nil)
    }

    @Test func withoutARepositoryTheUpdaterIsOff() async {
        let model = model(feed: .some(nil))
        #expect(!model.isEnabled)
        await model.check(userInitiated: true)
        #expect(model.phase == .idle)
    }

    @Test func installingStagesInstallsAndQuits() async throws {
        let model = model()
        var quit = false
        model.terminate = { quit = true }

        feed.releases = [Self.release("0.2.0")]
        await model.check(userInitiated: true)
        model.install()
        try await waitUntil { quit }

        #expect(installer.installed?.version.description == "0.2.0")
        #expect(model.phase == .installing(Self.release("0.2.0")))
    }

    @Test func aFailedInstallSaysWhyAndQuitsNothing() async throws {
        let model = model()
        var quit = false
        model.terminate = { quit = true }
        installer.stageError = UpdateInstallError.checksumMismatch
        feed.releases = [Self.release("0.2.0")]

        await model.check(userInitiated: true)
        model.install()
        try await waitUntil { if case .failed = model.phase { true } else { false } }

        #expect(!quit)
        #expect(installer.installed == nil)
    }

    private func waitUntil(_ condition: () -> Bool) async throws {
        for _ in 0..<200 where !condition() {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(condition())
    }
}

private final class TestClock: @unchecked Sendable {
    var now = Date(timeIntervalSince1970: 1_790_000_000)
}

private final class ReleaseFeedFake: ReleaseFeed, @unchecked Sendable {
    var releases: [AppRelease] = []
    var error: (any Error)?

    func releases() async throws -> [AppRelease] {
        if let error { throw error }
        return releases
    }
}

private final class UpdateInstallerFake: UpdateInstalling, @unchecked Sendable {
    var stageError: (any Error)?
    var installed: StagedUpdate?

    func installability() -> Installability { .available }

    func stage(_ release: AppRelease, progress: @escaping @Sendable (Double) -> Void) async throws -> StagedUpdate {
        progress(0.5)
        if let stageError { throw stageError }
        return StagedUpdate(app: URL(fileURLWithPath: "/tmp/PhoneLink.app"), version: release.version)
    }

    func install(_ staged: StagedUpdate) throws {
        installed = staged
    }
}
