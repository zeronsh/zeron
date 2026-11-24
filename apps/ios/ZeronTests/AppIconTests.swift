import XCTest
@testable import Zeron

final class AppIconTests: XCTestCase {
    func testLightIsThePrimaryIcon() {
        XCTAssertNil(AppIcon.light.alternateIconName)
        XCTAssertEqual(AppIcon.dark.alternateIconName, "AppIconDark")
    }

    func testChoiceFromTheIconInUse() {
        XCTAssertEqual(AppIcon(alternateIconName: nil), .light)
        XCTAssertEqual(AppIcon(alternateIconName: "AppIconDark"), .dark)
        // An icon this build no longer has: iOS shows the primary.
        XCTAssertEqual(AppIcon(alternateIconName: "AppIconGone"), .light)
    }

    /// Every choice is an icon the app was built with: `setAlternateIconName`
    /// fails for a name Info.plist doesn't list, and the picker shows each one.
    func testEveryChoiceIsInTheBundle() {
        let icons = Bundle.main.object(forInfoDictionaryKey: "CFBundleIcons") as? [String: Any]
        let alternates = icons?["CFBundleAlternateIcons"] as? [String: Any] ?? [:]
        XCTAssertEqual(Set(alternates.keys), Set(AppIcon.allCases.compactMap(\.alternateIconName)))
        for icon in AppIcon.allCases {
            XCTAssertNotNil(icon.preview, "\(icon)")
        }
    }
}
