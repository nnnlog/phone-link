import Foundation

/// The app's base bundle identifier, which every target's own identifier, app group, log subsystem,
/// queue label and temporary folder name start with. It is set once, as `PHONELINK_BUNDLE_ID` in
/// `Identity.xcconfig`, and each bundle's Info.plist carries it as `PhoneLinkBundleID`; this reads it
/// from the bundle this code is linked into (the app, an extension, or the test bundle).
enum PhoneLinkIdentity {
    static let bundleID: String = {
        let bundle = Bundle(for: BundleToken.self)
        guard let value = bundle.object(forInfoDictionaryKey: "PhoneLinkBundleID") as? String, !value.isEmpty else {
            preconditionFailure("\(bundle.bundlePath) has no PhoneLinkBundleID in its Info.plist")
        }
        return value
    }()

    private final class BundleToken {}
}
