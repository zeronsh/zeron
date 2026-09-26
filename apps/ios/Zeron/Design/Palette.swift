import UIKit

/// App palette. Light is a warm paper grey, dark is true black; accents are a
/// single teal. Colors are paint-only — nothing here affects layout.
enum Palette {
    static func dynamic(_ light: UInt32, _ dark: UInt32, alpha: CGFloat = 1) -> UIColor {
        UIColor { traits in
            UIColor(hex: traits.userInterfaceStyle == .dark ? dark : light, alpha: alpha)
        }
    }

    static let background = dynamic(0xEEEEEC, 0x000000)
    static let elevated = dynamic(0xFFFFFF, 0x161616)
    static let text = dynamic(0x161616, 0xF2F2F2)
    static let secondary = dynamic(0x7A7A78, 0x8F8F8F)
    static let tertiary = dynamic(0xA3A3A0, 0x5E5E5E)
    static let hairline = dynamic(0xD8D8D5, 0x262626)
    static let accent = dynamic(0x10988A, 0x2BB5A5)
    static let danger = dynamic(0xD2433B, 0xF0625A)
    static let success = dynamic(0x2E9A4E, 0x4CC46E)
    static let warning = dynamic(0xC9861A, 0xE8A93A)
    static let userBubble = dynamic(0xFFFFFF, 0x1C1C1C)
    static let codeBackground = dynamic(0xF6F6F4, 0x0E0E0E)
    static let codeBorder = dynamic(0xDEDEDA, 0x232323)
    static let chip = dynamic(0xE3E3E0, 0x1E1E1E)

    /// Stable project dot colors (capy-style muted hues).
    static let projectDots: [UIColor] = [
        dynamic(0x7F7A12, 0xB5AE3A), dynamic(0xB0406E, 0xD9689A), dynamic(0xB84A3E, 0xE07466),
        dynamic(0xB8612A, 0xE58B50), dynamic(0x9B4FB8, 0xC27BDD), dynamic(0x3F6FC4, 0x6E99E6),
        dynamic(0x3E8E57, 0x62B97C), dynamic(0x996F2A, 0xC8984C),
    ]

    static func color(_ role: ColorRole) -> UIColor {
        switch role {
        case .text: text
        case .textSecondary: secondary
        case .textTertiary: tertiary
        case .link, .accent: accent
        case .danger: danger
        case .success: success
        case .warning: warning
        case .inlineCodeText: dynamic(0x2C2C2C, 0xE6E6E6)
        case .inlineCodeBackground: dynamic(0xE2E2DF, 0x1F1F1F)
        case .codeText: dynamic(0x24292F, 0xD6D6D6)
        case .codeBackground: codeBackground
        case .codeBorder: codeBorder
        case .quoteBar: dynamic(0xCFCFCB, 0x333333)
        case .rule: hairline
        case .tableBorder: dynamic(0xD6D6D2, 0x2A2A2A)
        case .tableHeaderBackground: dynamic(0xE6E6E3, 0x141414)
        case .userBubble: userBubble
        case .chipBackground: chip
        case .syntaxKeyword: dynamic(0xB7355A, 0xFF7B9C)
        case .syntaxString: dynamic(0x1F7A3D, 0x8BD49C)
        case .syntaxComment: dynamic(0x8A8A86, 0x6A6A6A)
        case .syntaxNumber, .syntaxConstant: dynamic(0x0E6FA8, 0x79C0FF)
        case .syntaxFunction: dynamic(0x6F42C1, 0xD2A8FF)
        case .syntaxType: dynamic(0xA05A00, 0xFFB86C)
        case .syntaxVariable: dynamic(0x24292F, 0xD6D6D6)
        case .syntaxProperty: dynamic(0x0B5E8E, 0x9CDCFE)
        case .syntaxOperator, .syntaxPunctuation: dynamic(0x57606A, 0x9A9A9A)
        case .syntaxTag: dynamic(0x116329, 0x7EE787)
        case .syntaxAttribute: dynamic(0x953800, 0xFFA657)
        case .syntaxEscape: dynamic(0x0A6A6A, 0x56D4D4)
        }
    }
}

extension UIColor {
    convenience init(hex: UInt32, alpha: CGFloat = 1) {
        self.init(
            red: CGFloat((hex >> 16) & 0xFF) / 255,
            green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255,
            alpha: alpha
        )
    }
}
