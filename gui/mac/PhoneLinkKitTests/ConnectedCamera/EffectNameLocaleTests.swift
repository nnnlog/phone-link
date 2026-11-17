import Foundation
import Testing
@testable import PhoneLinkKit

@Test func effectNamesFollowTheSystemLocaleWhenTheAppRunsInItsLanguage() {
    #expect(ConnectedCameraModel.effectNameLocale(localization: "ko", current: Locale(identifier: "ko_KR")) == "ko-KR")
    #expect(ConnectedCameraModel.effectNameLocale(localization: "en", current: Locale(identifier: "en_GB")) == "en-GB")
}

@Test func effectNamesFollowTheAppWhenItFallsBackToAnotherLanguage() {
    #expect(ConnectedCameraModel.effectNameLocale(localization: "en", current: Locale(identifier: "de_DE")) == "en")
}

@Test func effectNamesFollowTheSystemLocaleWithoutALocalization() {
    #expect(ConnectedCameraModel.effectNameLocale(localization: nil, current: Locale(identifier: "ko_KR")) == "ko-KR")
}
