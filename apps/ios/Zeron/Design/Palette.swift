import UIKit

/// App palette. Light is a warm paper grey, dark is true black; accents are a
/// single teal. Colors are paint-only — nothing here affects layout.
enum Palette {
    static func dynamic(_ light: UInt32, _ dark: UInt32, alpha: CGFloat = 1) -> UIColor {
        UIColor { traits in
            UIColor(hex: traits.userInterfaceStyle == .dark ? dark : light, alpha: alpha)
        }
    }

    // Zeron Light / Zeron Dark (crates/theme builtins): cool neutrals, violet accent.
    static let background = dynamic(0xF3F3F5, 0x060606)
    /// The open session's row in the iPad sidebar.
    static let rowActive = dual(UIColor(white: 1, alpha: 0.9), UIColor(white: 1, alpha: 0.06))
    static let elevated = dynamic(0xFFFFFF, 0x111113)
    static let text = dynamic(0x27272C, 0xE8E8EA)
    static let secondary = dynamic(0x62626A, 0xA9A9AE)
    static let tertiary = dynamic(0x97979F, 0x6B6B72)
    static let hairline = dynamic(0xE2E2E6, 0x1E1E22)
    static let accent = dynamic(0x5B43E8, 0x8B7CF6)
    /// Translucent control fill that reads on glass in both modes.
    static let controlFill = dynamic(0x27272C, 0xE8E8EA, alpha: 0.075)
    static let accentSoft = dynamic(0x5B43E8, 0x8B7CF6, alpha: 0.12)
    static let danger = dynamic(0xDC2626, 0xF87171)
    static let success = dynamic(0x15803D, 0x34D399)
    static let warning = dynamic(0xA16207, 0xFACC15)
    static let userBubble = dynamic(0xFFFFFF, 0x19191C)
    static let codeBackground = dynamic(0xFAFAFB, 0x0B0B0D)
    static let codeBorder = dynamic(0xE4E4E8, 0x1F1F23)
    static let chip = dynamic(0xE7E7EB, 0x1C1C20)

    /// Project tones — the desktop's monogram palette (project_icon.rs):
    /// slate, blue, violet, rose, amber, emerald, teal, orange. Indexed by the
    /// core's `project_color_index` (FNV-1a of the project path).
    static let projectDots: [UIColor] = [
        dynamic(0x475569, 0x94A3B8), dynamic(0x2563EB, 0x93C5FD), dynamic(0x7C3AED, 0xC4B5FD),
        dynamic(0xBE123C, 0xFDA4AF), dynamic(0xA16207, 0xFCD34D), dynamic(0x047857, 0x6EE7B7),
        dynamic(0x0F766E, 0x5EEAD4), dynamic(0xC2410C, 0xFDBA74),
    ]

    /// Desktop sidebar "subline": `text_muted` @ 0.5 (project label, branch).
    static let subline = dynamic(0x62626A, 0xA9A9AE, alpha: 0.5)

    static func dual(_ light: UIColor, _ dark: UIColor) -> UIColor {
        UIColor { $0.userInterfaceStyle == .dark ? dark : light }
    }

    static func color(_ role: ColorRole) -> UIColor {
        switch role {
        case .text: text
        case .textSecondary: secondary
        case .textTertiary: tertiary
        case .link, .accent: accent
        case .danger: danger
        case .success: success
        case .warning: warning
        case .inlineCodeText: dynamic(0x3F3F46, 0xDCDCE0)
        case .inlineCodeBackground: dynamic(0xE9E9ED, 0x1A1A1E)
        case .codeText: dynamic(0x303035, 0xE8E8EA)
        case .codeBackground: codeBackground
        case .codeBorder: codeBorder
        case .quoteBar: dynamic(0x5B43E8, 0x8B7CF6, alpha: 0.45)
        case .rule: hairline
        case .tableBorder: dynamic(0xE2E2E6, 0x232327)
        case .tableHeaderBackground: dynamic(0xF3F3F5, 0x121215)
        case .userBubble: userBubble
        case .chipBackground: chip
        case .syntaxKeyword: dynamic(0x5B43E8, 0x8B7CF6)
        case .syntaxString: dynamic(0x15803D, 0x34D399)
        case .syntaxComment: dynamic(0x6B7280, 0x92929A)
        case .syntaxNumber, .syntaxConstant: dynamic(0xA16207, 0xFACC15)
        case .syntaxFunction: dynamic(0x2563EB, 0x60A5FA)
        case .syntaxType: dynamic(0x7E22CE, 0xC084FC)
        case .syntaxVariable: dynamic(0x303035, 0xE8E8EA)
        case .syntaxProperty: dynamic(0x0E7490, 0x22D3EE)
        case .syntaxOperator, .syntaxPunctuation: dynamic(0x52525B, 0xA1A1AA)
        case .syntaxTag: dynamic(0xBE185D, 0xF472B6)
        case .syntaxAttribute: dynamic(0xB91C1C, 0xF87171)
        case .syntaxEscape: dynamic(0x0E7490, 0x22D3EE)
        // Tool groups — desktop zeron tokens (see tools.rs).
        case .textFaint: dynamic(0x797981, 0x85858A)
        case .textSoft: dynamic(0x303035, 0xE8E8EA, alpha: 0.85)
        case .toolRail: dual(UIColor(white: 0, alpha: 0.162), UIColor(white: 1, alpha: 0.12))
        case .toolBadge: dual(UIColor(white: 0, alpha: 0.06), UIColor(white: 1, alpha: 0.06))
        case .toolWell: dual(UIColor(white: 1, alpha: 0.16), UIColor(white: 0, alpha: 0.16))
        case .agentCard: dual(UIColor(white: 0, alpha: 0.03), UIColor(white: 1, alpha: 0.03))
        case .agentCardBorder: dual(UIColor(white: 0, alpha: 0.0945), UIColor(white: 1, alpha: 0.07))
        case .agentTile: dual(UIColor(white: 0, alpha: 0.08), UIColor(white: 1, alpha: 0.08))
        case .diffAddWash: dynamic(0x15803D, 0x34D399, alpha: 0.055)
        case .diffDelWash: dynamic(0xDC2626, 0xF87171, alpha: 0.055)
        case .diffAddBar: dynamic(0x15803D, 0x34D399, alpha: 0.55)
        case .diffDelBar: dynamic(0xDC2626, 0xF87171, alpha: 0.55)
        case .diffHunk: dual(UIColor(hex: 0x5B43E8, alpha: 0.07), UIColor(hex: 0x8B7CF6, alpha: 0.08))
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

/// Dynamic Type for fixed-height UI: one factor per content size category
/// (rows stay fixed-height, so lists still skip self-sizing).
enum TypeScale {
    static var factor: CGFloat {
        min(1.6, max(0.85, UIFontMetrics(forTextStyle: .body).scaledValue(for: 17) / 17))
    }

    static func size(_ base: CGFloat) -> CGFloat { (base * factor).rounded(.toNearestOrAwayFromZero) }
}
