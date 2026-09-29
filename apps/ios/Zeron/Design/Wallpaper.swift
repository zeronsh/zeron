import ImageIO
import PhotosUI
import UIKit
import UniformTypeIdentifiers

/// The chat wallpaper (desktop "new thread background"): one image plus an
/// effect, stored locally. Effects and the contrast guard run in the Rust core
/// (`wallpaper.rs`) off the main thread; renders are cached per appearance.
enum WallpaperStore {
    static let didChange = Notification.Name("ZeronWallpaperChanged")

    private static var directory: URL {
        let dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("wallpaper", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    private static var imageURL: URL { directory.appendingPathComponent("wallpaper.jpg") }

    static var isSet: Bool { FileManager.default.fileExists(atPath: imageURL.path) }
    static var name: String? { UserDefaults.standard.string(forKey: "wallpaperName") }

    static var effect: WallpaperEffect {
        get {
            switch UserDefaults.standard.string(forKey: "wallpaperEffect") {
            case "dither": .dither
            case "ascii": .ascii
            case "halftone": .halftone
            case "scanlines": .scanlines
            default: .none
            }
        }
        set {
            UserDefaults.standard.set(key(newValue), forKey: "wallpaperEffect")
            changed()
        }
    }

    static func key(_ e: WallpaperEffect) -> String {
        switch e {
        case .none: "none"
        case .dither: "dither"
        case .ascii: "ascii"
        case .halftone: "halftone"
        case .scanlines: "scanlines"
        }
    }

    static let allEffects: [WallpaperEffect] = [.none, .dither, .ascii, .halftone, .scanlines]

    /// Desktop labels / descriptions.
    static func label(_ e: WallpaperEffect) -> String {
        switch e {
        case .none: "None"
        case .dither: "Dither"
        case .ascii: "ASCII"
        case .halftone: "Halftone"
        case .scanlines: "Scanlines"
        }
    }

    static func detail(_ e: WallpaperEffect) -> String {
        switch e {
        case .none: "Shows the original artwork."
        case .dither: "Rebuilds the artwork with a dithered color palette."
        case .ascii: "Recreates the artwork with colored characters."
        case .halftone: "Recreates the artwork with colored print dots."
        case .scanlines: "Adds a pronounced horizontal display-line texture."
        }
    }

    /// Store a picked image (long side ≤ 1600 px) and notify.
    static func set(_ image: UIImage, name: String) {
        let scale = min(1, 1600 / max(image.size.width * image.scale, image.size.height * image.scale))
        let size = CGSize(width: (image.size.width * image.scale * scale).rounded(), height: (image.size.height * image.scale * scale).rounded())
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let resized = UIGraphicsImageRenderer(size: size, format: format).image { _ in image.draw(in: CGRect(origin: .zero, size: size)) }
        try? resized.jpegData(compressionQuality: 0.9)?.write(to: imageURL, options: .atomic)
        UserDefaults.standard.set(name, forKey: "wallpaperName")
        changed()
    }

    /// Store a picked photo straight from its file, downsampled while it's
    /// decoded (a 48MP photo never becomes a full-size bitmap), off the main
    /// thread; notifies on main.
    static func set(fileURL: URL, name: String) {
        let options = [kCGImageSourceShouldCache: false] as CFDictionary
        guard let source = CGImageSourceCreateWithURL(fileURL as CFURL, options),
              let thumb = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                  kCGImageSourceCreateThumbnailFromImageAlways: true,
                  kCGImageSourceCreateThumbnailWithTransform: true,
                  kCGImageSourceThumbnailMaxPixelSize: 1600,
              ] as CFDictionary),
              let data = UIImage(cgImage: thumb).jpegData(compressionQuality: 0.9)
        else { return }
        try? data.write(to: imageURL, options: .atomic)
        DispatchQueue.main.async {
            UserDefaults.standard.set(name, forKey: "wallpaperName")
            changed()
        }
    }

    static func remove() {
        try? FileManager.default.removeItem(at: imageURL)
        UserDefaults.standard.removeObject(forKey: "wallpaperName")
        changed()
    }

    private static func changed() {
        lock.lock()
        cache.removeAll()
        generation += 1
        lock.unlock()
        NotificationCenter.default.post(name: didChange, object: nil)
    }

    /// A rendered wallpaper and the highest opacity that keeps text readable.
    struct Render {
        let image: UIImage
        let opacity: CGFloat
    }

    private static let lock = NSLock()
    private static var cache: [String: Render] = [:]
    private static var generation = 0
    private static let queue = DispatchQueue(label: "sh.zeron.wallpaper", qos: .userInitiated)

    /// Render for an appearance; `completion` runs on the main thread (nil
    /// when no wallpaper is set). Cached until the image or effect changes.
    static func render(dark: Bool, completion: @escaping (Render?) -> Void) {
        guard isSet else { return completion(nil) }
        let effect = self.effect
        let key = "\(Self.key(effect))-\(dark)"
        lock.lock()
        let hit = cache[key]
        let gen = generation
        lock.unlock()
        if let hit { return completion(hit) }
        let traits = UITraitCollection(userInterfaceStyle: dark ? .dark : .light)
        let text = rgb(Palette.text.resolvedColor(with: traits))
        let secondary = rgb(Palette.secondary.resolvedColor(with: traits))
        let background = rgb(Palette.background.resolvedColor(with: traits))
        let url = imageURL
        queue.async {
            guard let source = UIImage(contentsOfFile: url.path)?.cgImage,
                  let (pixels, w, h) = rgba(source, maxSide: 1200)
            else { return DispatchQueue.main.async { completion(nil) } }
            let out = wallpaperRender(rgba: pixels, width: UInt32(w), height: UInt32(h), effect: effect, light: !dark)
            // Text sits over the top of the artwork: primary text keeps 4.5:1,
            // secondary 3:1 against the artwork's worst pixels there.
            let region: Float = 0.6
            let primary = wallpaperSafeOpacity(rgba: out, width: UInt32(w), height: UInt32(h), textRgb: text, backgroundRgb: background, region: region, minContrast: 4.5, maxOpacity: 1)
            let muted = wallpaperSafeOpacity(rgba: out, width: UInt32(w), height: UInt32(h), textRgb: secondary, backgroundRgb: background, region: region, minContrast: 3, maxOpacity: 1)
            guard let image = cgImage(out, w, h) else { return DispatchQueue.main.async { completion(nil) } }
            let render = Render(image: UIImage(cgImage: image), opacity: CGFloat(min(primary, muted)))
            lock.lock()
            if gen == generation { cache[key] = render }
            lock.unlock()
            DispatchQueue.main.async { completion(render) }
        }
    }

    private static func rgb(_ c: UIColor) -> UInt32 {
        var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
        c.getRed(&r, green: &g, blue: &b, alpha: &a)
        return UInt32(r * 255) << 16 | UInt32(g * 255) << 8 | UInt32(b * 255)
    }

    private static func rgba(_ image: CGImage, maxSide: CGFloat) -> (Data, Int, Int)? {
        let scale = min(1, maxSide / CGFloat(max(image.width, image.height)))
        let w = max(1, Int(CGFloat(image.width) * scale)), h = max(1, Int(CGFloat(image.height) * scale))
        var data = Data(count: w * h * 4)
        let ok = data.withUnsafeMutableBytes { buf -> Bool in
            guard let ctx = CGContext(data: buf.baseAddress, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
            ctx.interpolationQuality = .high
            ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
            return true
        }
        return ok ? (data, w, h) : nil
    }

    private static func cgImage(_ data: Data, _ w: Int, _ h: Int) -> CGImage? {
        guard let provider = CGDataProvider(data: data as CFData) else { return nil }
        return CGImage(width: w, height: h, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: w * 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue), provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    }
}

/// The wallpaper hero: aspect-filled artwork at the top of a page that fades
/// out downward (alpha, never a tinted overlay — the page shows through), at
/// the opacity the contrast guard allows. An optional cutout (the new-chat
/// composer) softens the artwork behind it, like desktop.
final class WallpaperView: UIView {
    private let imageView = UIImageView()
    private let maskLayer = CALayer()
    private var render: WallpaperStore.Render?
    private var observer: NSObjectProtocol?
    /// Extra fade applied by the host (e.g. scrolling the list away).
    var scrollFade: CGFloat = 1 { didSet { applyAlpha() } }
    /// A rect (in this view's coordinates) to soften — the composer.
    var cutout: CGRect? { didSet { if cutout != oldValue { setNeedsLayout() } } }

    override init(frame: CGRect) {
        super.init(frame: frame)
        isUserInteractionEnabled = false
        clipsToBounds = true
        imageView.contentMode = .scaleAspectFill
        imageView.clipsToBounds = true
        addSubview(imageView)
        layer.mask = maskLayer
        alpha = 0
        observer = NotificationCenter.default.addObserver(forName: WallpaperStore.didChange, object: nil, queue: .main) { [weak self] _ in self?.reload() }
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (self: WallpaperView, _) in self.reload() }
        accessibilityIdentifier = "wallpaper"
    }

    required init?(coder: NSCoder) { fatalError() }

    deinit { observer.map(NotificationCenter.default.removeObserver) }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window != nil, render == nil { reload() }
    }

    func reload() {
        let dark = traitCollection.userInterfaceStyle == .dark
        WallpaperStore.render(dark: dark) { [weak self] render in
            guard let self else { return }
            let first = self.render == nil
            self.render = render
            self.imageView.image = render?.image
            self.isHidden = render == nil
            // Artwork arriving late fades in (desktop: 120 ms).
            if first, render != nil, window != nil, !UIAccessibility.isReduceMotionEnabled {
                self.alpha = 0
                UIView.animate(withDuration: 0.12) { self.applyAlpha() }
            } else {
                self.applyAlpha()
            }
        }
    }

    private func applyAlpha() {
        alpha = (render?.opacity ?? 0) * max(0, min(1, scrollFade))
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        imageView.frame = bounds
        maskLayer.frame = bounds
        maskLayer.contents = Self.mask(size: bounds.size, cutout: cutout)
    }

    /// Alpha mask: full at the top fading to clear at the bottom (eased, over
    /// the full height as on desktop), halved inside a feathered cutout.
    private static func mask(size: CGSize, cutout: CGRect?) -> CGImage? {
        guard size.width > 0, size.height > 0 else { return nil }
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        format.opaque = false
        return UIGraphicsImageRenderer(size: size, format: format).image { ctx in
            let c = ctx.cgContext
            let steps = 24
            var colors: [CGColor] = []
            var locations: [CGFloat] = []
            for i in 0...steps {
                let t = CGFloat(i) / CGFloat(steps)
                // Smoothstep-ish ease so the fade has no visible band.
                let a = 1 - (t * t * (3 - 2 * t))
                colors.append(UIColor(white: 0, alpha: a).cgColor)
                locations.append(t)
            }
            if let g = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: colors as CFArray, locations: locations) {
                c.drawLinearGradient(g, start: .zero, end: CGPoint(x: 0, y: size.height), options: [])
            }
            if let cut = cutout, cut.intersects(CGRect(origin: .zero, size: size)) {
                let feather = min(280, max(120, size.height * 0.52)) / 3
                c.setBlendMode(.destinationOut)
                c.setShadow(offset: .zero, blur: feather, color: UIColor(white: 0, alpha: 0.5).cgColor)
                UIColor(white: 0, alpha: 0.5).setFill()
                UIBezierPath(roundedRect: cut.insetBy(dx: -8, dy: -8), cornerRadius: 26).fill()
            }
        }.cgImage
    }
}

/// Picks a wallpaper image from Photos.
final class WallpaperPicker: NSObject, PHPickerViewControllerDelegate {
    private var retainSelf: WallpaperPicker?

    static func present(from host: UIViewController) {
        var config = PHPickerConfiguration()
        config.filter = .images
        config.selectionLimit = 1
        let picker = PHPickerViewController(configuration: config)
        let delegate = WallpaperPicker()
        delegate.retainSelf = delegate
        picker.delegate = delegate
        host.present(picker, animated: true)
    }

    func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
        picker.dismiss(animated: true)
        guard let provider = results.first?.itemProvider, provider.hasItemConformingToTypeIdentifier(UTType.image.identifier) else {
            retainSelf = nil
            return
        }
        let name = provider.suggestedName ?? "Photo"
        // The file, not a decoded UIImage: it's downsampled as it's read.
        provider.loadFileRepresentation(forTypeIdentifier: UTType.image.identifier) { [weak self] url, _ in
            if let url { WallpaperStore.set(fileURL: url, name: name) }
            self?.retainSelf = nil
        }
    }
}
