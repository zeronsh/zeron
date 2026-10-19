import SwiftUI
import UIKit

/// The app's faces (Geist), bundled with the extension.
enum Face {
    static func sans(_ size: CGFloat) -> Font { .custom("Geist-Medium", size: size) }
    static func mono(_ size: CGFloat) -> Font { .custom("GeistMono-Medium", size: size) }
}

/// Zeron Dark/Light colors (`Palette`, `StatusTone`), as the app paints them.
enum Tone {
    static let accent = dynamic(0x5B43E8, 0x8B7CF6)
    static let warning = dynamic(0xA16207, 0xFACC15)
    static let danger = dynamic(0xDC2626, 0xF87171)
    static let success = dynamic(0x15803D, 0x34D399)

    private static func dynamic(_ light: UInt32, _ dark: UInt32) -> Color {
        Color(UIColor { $0.userInterfaceStyle == .dark ? UIColor(hex: dark) : UIColor(hex: light) })
    }
}

extension UIColor {
    convenience init(hex: UInt32) {
        self.init(
            red: CGFloat(hex >> 16 & 0xFF) / 255,
            green: CGFloat(hex >> 8 & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255,
            alpha: 1
        )
    }
}
