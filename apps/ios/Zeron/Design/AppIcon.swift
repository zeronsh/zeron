import UIKit

/// The Home Screen icons Settings offers. Light is the app's primary icon; each
/// other is an icon set of its own in the asset catalog, named in the target's
/// ASSETCATALOG_COMPILER_ALTERNATE_APPICON_NAMES.
enum AppIcon: String, CaseIterable {
    case light
    case dark

    var title: String {
        switch self {
        case .light: "Light"
        case .dark: "Dark"
        }
    }

    /// What `setAlternateIconName` takes: the icon set's name, nil for the primary.
    var alternateIconName: String? {
        switch self {
        case .light: nil
        case .dark: "AppIconDark"
        }
    }

    /// The choice whose icon is named `name`. A name none has (an icon a later
    /// build dropped) is the primary: iOS shows that one in its place.
    init(alternateIconName name: String?) {
        self = Self.allCases.first { $0.alternateIconName == name } ?? .light
    }

    /// The one on the Home Screen now.
    static var current: AppIcon { AppIcon(alternateIconName: UIApplication.shared.alternateIconName) }

    /// A picture of the icon, corners still square. An app can't load its own
    /// icon sets as images, so each has an image set beside it
    /// (scripts/make-app-icons.py writes both).
    var preview: UIImage? { UIImage(named: "app-icon-\(rawValue)") }
}
