import CoreText
import UIKit

/// A terminal grid on screen. The emulator lives in Rust (`TerminalSession`);
/// this paints its frames on a monospace grid and turns touches and keys
/// into input.
///
/// It is a scroll view so scrollback feels native — momentum, rubber band,
/// the indicator — while the painted grid stays put: the content height is
/// the scrollback, and the offset picks how far back the grid shows.
final class TerminalView: UIScrollView, UIKeyInput, UITextInputTraits, UIScrollViewDelegate, UIEditMenuInteractionDelegate {
    let tab: TerminalTab
    private var session: TerminalSession { tab.session }
    private let canvas = TerminalCanvas()
    private let cursor = CALayer()
    private var token: AnyObject?
    private(set) var frameModel: TerminalFrame?
    /// Sticky modifiers from the key bar, applied to the next key.
    var modifiers: () -> KeyModifiers = { KeyModifiers(ctrl: false, alt: false, shift: false) }
    var consumeModifiers: () -> Void = {}
    /// Selected text to put in the chat's draft.
    var onAddToMessage: ((String) -> Void)?
    var onFirstResponderChange: ((Bool) -> Void)?
    private lazy var editMenu = UIEditMenuInteraction(delegate: self)

    static let fontSizeKey = "terminalFontSize"
    static var defaultFontSize: CGFloat { UIDevice.current.userInterfaceIdiom == .pad ? 13.5 : 12 }
    private(set) var fontSize: CGFloat = {
        let saved = UserDefaults.standard.double(forKey: TerminalView.fontSizeKey)
        return saved > 0 ? CGFloat(saved) : TerminalView.defaultFontSize
    }()

    private var metrics = TerminalMetrics(size: 12)
    /// Grid insets inside the view.
    static let inset = UIEdgeInsets(top: 6, left: 10, bottom: 6, right: 8)

    init(tab: TerminalTab) {
        self.tab = tab
        super.init(frame: .zero)
        backgroundColor = .clear
        delegate = self
        alwaysBounceVertical = true
        showsHorizontalScrollIndicator = false
        contentInsetAdjustmentBehavior = .never
        keyboardDismissMode = .none
        delaysContentTouches = false
        canvas.isUserInteractionEnabled = false
        addSubview(canvas)
        cursor.actions = ["position": NSNull(), "bounds": NSNull(), "frame": NSNull()]
        cursor.cornerRadius = 1
        canvas.layer.addSublayer(cursor)
        metrics = TerminalMetrics(size: fontSize)
        canvas.metrics = metrics

        addGestureRecognizer(UITapGestureRecognizer(target: self, action: #selector(tapped(_:))))
        let press = UILongPressGestureRecognizer(target: self, action: #selector(pressed(_:)))
        press.minimumPressDuration = 0.35
        addGestureRecognizer(press)
        addGestureRecognizer(UIPinchGestureRecognizer(target: self, action: #selector(pinched(_:))))
        altPan.addTarget(self, action: #selector(altPanned(_:)))
        altPan.maximumNumberOfTouches = 1
        altPan.isEnabled = false
        addGestureRecognizer(altPan)
        addInteraction(editMenu)
        isAccessibilityElement = true
        accessibilityTraits = .allowsDirectInteraction
        accessibilityIdentifier = "terminal-view"

        token = tab.observe { [weak self] in self?.refresh() }
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (view: TerminalView, _) in
            view.session.setDark(dark: view.traitCollection.userInterfaceStyle == .dark)
            view.canvas.setNeedsDisplay()
        }
        refresh()
    }

    required init?(coder: NSCoder) { fatalError() }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window != nil { session.setDark(dark: traitCollection.userInterfaceStyle == .dark) }
    }

    // MARK: Frames

    private var followingBottom = true

    private func refresh() {
        let frame = session.frame()
        frameModel = frame
        canvas.frameModel = frame
        canvas.setNeedsDisplay()
        if UIAccessibility.isVoiceOverRunning { accessibilityValue = session.text(all: false) }
        syncScroll(frame)
        placeCursor(frame)
    }

    /// Content height = scrollback + viewport; the offset mirrors how far
    /// back the grid shows. On the alternate screen there's nothing to
    /// scroll back to (the wheel is the program's).
    private func syncScroll(_ frame: TerminalFrame) {
        let lineH = metrics.lineHeight
        // A full-screen program (or one tracking the pointer) owns the wheel.
        let programWheel = frame.altScreen || frame.mouse
        if altPan.isEnabled != programWheel {
            altPan.isEnabled = programWheel
            isScrollEnabled = !programWheel
        }
        let history = frame.altScreen ? 0 : CGFloat(frame.history)
        let height = bounds.height + history * lineH
        if abs(contentSize.height - height) > 0.5 { contentSize = CGSize(width: bounds.width, height: height) }
        guard !isTracking, !isDecelerating else { return }
        let target = max(0, (history - CGFloat(frame.displayOffset) - CGFloat(frame.scrollFrac)) * lineH)
        if abs(contentOffset.y - target) > 0.5 {
            applying = true
            contentOffset.y = target
            applying = false
        }
    }

    private var applying = false

    func scrollViewDidScroll(_ scrollView: UIScrollView) {
        // The grid stays in the viewport; only what it shows changes.
        canvas.frame = CGRect(origin: CGPoint(x: 0, y: contentOffset.y), size: bounds.size)
        guard !applying, let frame = frameModel, !frame.altScreen else { return }
        // By the pixel: the core keeps the fraction of a row, and the grid
        // paints that much lower with the revealed row above.
        let history = CGFloat(frame.history)
        let back = max(0, min(history, history - contentOffset.y / metrics.lineHeight))
        session.scrollToPosition(back: Float(back))
    }

    /// One-finger drags on a full-screen program become its wheel.
    private let altPan = UIPanGestureRecognizer()
    private var altScrolled: CGFloat = 0

    @objc private func altPanned(_ g: UIPanGestureRecognizer) {
        switch g.state {
        case .began:
            altScrolled = 0
        case .changed:
            // Rows as a fraction: the core owes the program whole ones.
            let rows = g.translation(in: self).y / metrics.lineHeight
            let c = cell(at: g.location(in: self))
            _ = session.wheel(rows: Float(rows - altScrolled), row: c.row, col: c.col)
            altScrolled = rows
        default:
            break
        }
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        canvas.frame = CGRect(origin: CGPoint(x: 0, y: contentOffset.y), size: bounds.size)
        scheduleResize()
        if let frame = frameModel {
            syncScroll(frame)
            placeCursor(frame)
        }
    }

    // MARK: Grid size

    private var resizeWork: DispatchWorkItem?
    private var lastGrid: (UInt16, UInt16)?

    var grid: (cols: UInt16, rows: UInt16) {
        let w = bounds.width - Self.inset.left - Self.inset.right
        let h = bounds.height - Self.inset.top - Self.inset.bottom
        return (UInt16(max(20, floor(w / metrics.cellWidth))), UInt16(max(4, floor(h / metrics.lineHeight))))
    }

    /// The host follows the view's size once it settles (a keyboard sliding
    /// in mustn't resize the PTY a dozen times).
    private func scheduleResize() {
        guard bounds.width > 40, bounds.height > 40 else { return }
        let g = grid
        if let last = lastGrid, last == (g.cols, g.rows) { return }
        resizeWork?.cancel()
        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            let g = self.grid
            self.lastGrid = (g.cols, g.rows)
            self.session.resize(cols: g.cols, rows: g.rows)
        }
        resizeWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + (lastGrid == nil ? 0 : 0.08), execute: work)
    }

    // MARK: Cursor

    private func placeCursor(_ frame: TerminalFrame) {
        guard let c = frame.cursor, frame.displayOffset == 0 else {
            cursor.isHidden = true
            return
        }
        cursor.isHidden = false
        let x = Self.inset.left + CGFloat(c.col) * metrics.cellWidth
        let y = Self.inset.top + CGFloat(c.row) * metrics.lineHeight
        let focused = isFirstResponder
        let tint = Palette.accent.resolvedColor(with: traitCollection)
        cursor.borderWidth = 0
        cursor.backgroundColor = tint.withAlphaComponent(0.55).cgColor
        switch c.shape {
        case .beam:
            cursor.frame = CGRect(x: x, y: y + 1, width: 2, height: metrics.lineHeight - 2)
            cursor.backgroundColor = tint.cgColor
        case .underline:
            cursor.frame = CGRect(x: x, y: y + metrics.lineHeight - 2, width: metrics.cellWidth, height: 2)
            cursor.backgroundColor = tint.cgColor
        case .block, .hollowBlock:
            cursor.frame = CGRect(x: x, y: y + 1, width: metrics.cellWidth, height: metrics.lineHeight - 2)
            if c.shape == .hollowBlock || !focused {
                cursor.backgroundColor = UIColor.clear.cgColor
                cursor.borderColor = tint.withAlphaComponent(0.8).cgColor
                cursor.borderWidth = 1
            }
        }
        // The blink rests after a while without typing (an idle terminal
        // schedules no frames); the next key resumes it.
        let resting = Date().timeIntervalSince(lastKey) > Self.blinkIdle
        let blink = focused && frame.cursorBlinks && !resting && !UIAccessibility.isReduceMotionEnabled
        if blink, cursor.animation(forKey: "blink") == nil {
            let anim = CAKeyframeAnimation(keyPath: "opacity")
            anim.values = [1, 1, 0, 0, 1]
            anim.keyTimes = [0, 0.45, 0.5, 0.95, 1]
            anim.duration = 1.1
            anim.repeatCount = .infinity
            cursor.add(anim, forKey: "blink")
        } else if !blink {
            cursor.removeAnimation(forKey: "blink")
        }
        // Typing restarts the blink in its visible phase.
        if blink, typedRecently {
            cursor.removeAnimation(forKey: "blink")
            typedRecently = false
            placeCursor(frame)
        }
    }

    private var typedRecently = false {
        didSet { if typedRecently { lastKey = Date(); scheduleBlinkRest() } }
    }
    private var lastKey = Date()
    private var blinkRest: DispatchWorkItem?
    static let blinkIdle: TimeInterval = 15

    private func scheduleBlinkRest() {
        blinkRest?.cancel()
        let work = DispatchWorkItem { [weak self] in
            guard let self, let frame = self.frameModel else { return }
            self.placeCursor(frame)
        }
        blinkRest = work
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.blinkIdle + 0.1, execute: work)
    }

    // MARK: Gestures

    @objc private func tapped(_ g: UITapGestureRecognizer) {
        if session.selection() != nil {
            session.clearSelection()
            editMenu.dismissMenu()
            return
        }
        // A program tracking the pointer (htop, vim with the mouse on) gets
        // the tap as a click.
        if frameModel?.mouse == true {
            let c = cell(at: g.location(in: self))
            _ = session.click(row: c.row, col: c.col)
        }
        if !isFirstResponder { becomeFirstResponder() }
    }

    private func cell(at point: CGPoint) -> (row: UInt16, col: UInt16) {
        let p = CGPoint(x: point.x, y: point.y - contentOffset.y)
        let col = max(0, (p.x - Self.inset.left) / metrics.cellWidth)
        let row = max(0, (p.y - Self.inset.top) / metrics.lineHeight)
        let g = frameModel.map { (Int($0.cols), Int($0.rows)) } ?? (80, 24)
        return (UInt16(min(Int(row), g.1 - 1)), UInt16(min(Int(col), g.0 - 1)))
    }

    @objc private func pressed(_ g: UILongPressGestureRecognizer) {
        let point = g.location(in: self)
        let c = cell(at: point)
        switch g.state {
        case .began:
            UISelectionFeedbackGenerator().selectionChanged()
            editMenu.dismissMenu()
            session.select(row: c.row, col: c.col, word: true)
        case .changed:
            session.extendSelection(row: c.row, col: c.col)
        case .ended:
            showSelectionMenu()
        default:
            break
        }
    }

    private func showSelectionMenu() {
        guard let selection = session.selection() else { return }
        let x = Self.inset.left + CGFloat(selection.startCol + selection.endCol) / 2 * metrics.cellWidth
        let y = Self.inset.top + CGFloat(selection.startRow) * metrics.lineHeight + contentOffset.y
        let config = UIEditMenuConfiguration(identifier: nil, sourcePoint: CGPoint(x: x, y: y))
        config.preferredArrowDirection = .down
        editMenu.presentEditMenu(with: config)
    }

    func editMenuInteraction(_ interaction: UIEditMenuInteraction, menuFor configuration: UIEditMenuConfiguration, suggestedActions: [UIMenuElement]) -> UIMenu? {
        guard let text = session.selection()?.text else { return nil }
        var actions: [UIMenuElement] = [
            UIAction(title: "Copy", image: UIImage(systemName: "doc.on.doc")) { [weak self] _ in
                UIPasteboard.general.string = text
                self?.flashSelection()
            },
        ]
        if onAddToMessage != nil {
            actions.append(UIAction(title: "Add to Message", image: UIImage(systemName: "text.bubble")) { [weak self] _ in
                self?.onAddToMessage?(text)
                self?.session.clearSelection()
            })
        }
        actions.append(UIAction(title: "Paste", image: UIImage(systemName: "doc.on.clipboard")) { [weak self] _ in
            self?.session.clearSelection()
            self?.pasteFromClipboard()
        })
        return UIMenu(children: actions)
    }

    /// The copy flash (the desktop's): the selection brightens, then lets go.
    private func flashSelection() {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        canvas.selectionBoost = 1
        canvas.setNeedsDisplay()
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.18) { [weak self] in
            self?.canvas.selectionBoost = 0
            self?.session.clearSelection()
        }
    }

    private var pinchBase: CGFloat = 12

    @objc private func pinched(_ g: UIPinchGestureRecognizer) {
        switch g.state {
        case .began:
            pinchBase = fontSize
        case .changed:
            let size = min(22, max(8, (pinchBase * g.scale * 2).rounded() / 2))
            if size != fontSize {
                if size == 8 || size == 22 { UIImpactFeedbackGenerator(style: .rigid).impactOccurred() } else { UISelectionFeedbackGenerator().selectionChanged() }
                setFontSize(size)
            }
        default:
            break
        }
    }

    func setFontSize(_ size: CGFloat) {
        fontSize = size
        UserDefaults.standard.set(Double(size), forKey: Self.fontSizeKey)
        metrics = TerminalMetrics(size: size)
        canvas.metrics = metrics
        canvas.setNeedsDisplay()
        setNeedsLayout()
    }

    // MARK: Keyboard (UIKeyInput)

    /// The key bar, above the software keyboard.
    var accessory: UIView?
    override var inputAccessoryView: UIView? { accessory }

    override var canBecomeFirstResponder: Bool { true }

    override func becomeFirstResponder() -> Bool {
        let became = super.becomeFirstResponder()
        if became {
            session.setFocused(focused: true)
            typedRecently = true
            onFirstResponderChange?(true)
            if let frame = frameModel { placeCursor(frame) }
        }
        return became
    }

    override func resignFirstResponder() -> Bool {
        let resigned = super.resignFirstResponder()
        if resigned {
            session.setFocused(focused: false)
            onFirstResponderChange?(false)
            if let frame = frameModel { placeCursor(frame) }
        }
        return resigned
    }

    var hasText: Bool { true }

    func insertText(_ text: String) {
        typedRecently = true
        session.typeText(text: text, modifiers: modifiers())
        consumeModifiers()
    }

    func deleteBackward() {
        typedRecently = true
        session.press(key: .backspace, modifiers: modifiers())
        consumeModifiers()
    }

    func press(_ key: TerminalKey) {
        typedRecently = true
        session.press(key: key, modifiers: modifiers())
        consumeModifiers()
    }

    func pasteFromClipboard() {
        guard let text = UIPasteboard.general.string, !text.isEmpty else { return }
        session.paste(text: text)
    }

    override func paste(_ sender: Any?) { pasteFromClipboard() }

    override func canPerformAction(_ action: Selector, withSender sender: Any?) -> Bool {
        if action == #selector(paste(_:)) { return UIPasteboard.general.hasStrings }
        if action == #selector(copy(_:)) { return session.selection() != nil }
        return super.canPerformAction(action, withSender: sender)
    }

    override func copy(_ sender: Any?) {
        UIPasteboard.general.string = session.selection()?.text
    }

    // Plain text, no autocorrect: a terminal wants exactly what was typed.
    var autocorrectionType: UITextAutocorrectionType = .no
    var autocapitalizationType: UITextAutocapitalizationType = .none
    var spellCheckingType: UITextSpellCheckingType = .no
    var smartQuotesType: UITextSmartQuotesType = .no
    var smartDashesType: UITextSmartDashesType = .no
    var smartInsertDeleteType: UITextSmartInsertDeleteType = .no
    var keyboardType: UIKeyboardType = .asciiCapable
    var keyboardAppearance: UIKeyboardAppearance = .default
    var returnKeyType: UIReturnKeyType = .default
    var inlinePredictionType: UITextInlinePredictionType = .no

    // MARK: Hardware keyboard

    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        var unhandled = Set<UIPress>()
        for press in presses {
            guard let key = press.key, handle(key) else {
                unhandled.insert(press)
                continue
            }
        }
        if !unhandled.isEmpty { super.pressesBegan(unhandled, with: event) }
    }

    /// Keys the text system wouldn't deliver as text (arrows, Esc, Ctrl/Alt
    /// combos). Plain characters fall through to `insertText`.
    private func handle(_ key: UIKey) -> Bool {
        let flags = key.modifierFlags
        if flags.contains(.command) { return false }
        let mods = KeyModifiers(ctrl: flags.contains(.control), alt: flags.contains(.alternate), shift: flags.contains(.shift))
        let named: TerminalKey? = switch key.keyCode {
        case .keyboardUpArrow: .up
        case .keyboardDownArrow: .down
        case .keyboardLeftArrow: .left
        case .keyboardRightArrow: .right
        case .keyboardEscape: .escape
        case .keyboardHome: .home
        case .keyboardEnd: .end
        case .keyboardPageUp: .pageUp
        case .keyboardPageDown: .pageDown
        case .keyboardDeleteForward: .delete
        case .keyboardTab: .tab
        default: nil
        }
        typedRecently = true
        if let named {
            session.press(key: named, modifiers: mods)
            return true
        }
        if mods.ctrl || mods.alt {
            let text = key.charactersIgnoringModifiers
            guard !text.isEmpty else { return false }
            session.typeText(text: text, modifiers: KeyModifiers(ctrl: mods.ctrl, alt: mods.alt, shift: false))
            return true
        }
        return false
    }
}

/// Cell geometry for one font size (Geist Mono's advance, a roomy line).
struct TerminalMetrics {
    let size: CGFloat
    let regular: CTFont
    let bold: CTFont
    let italic: CTFont
    let cellWidth: CGFloat
    let lineHeight: CGFloat
    let ascent: CGFloat

    init(size: CGFloat) {
        self.size = size
        regular = Fonts.ctFont(.mono, size: Float(size))
        bold = Fonts.ctFont(.monoSemibold, size: Float(size))
        italic = Fonts.ctFont(.monoItalic, size: Float(size))
        var glyph = CGGlyph()
        var ch: UniChar = 0x4D // "M"
        CTFontGetGlyphsForCharacters(regular, &ch, &glyph, 1)
        var advance = CGSize.zero
        CTFontGetAdvancesForGlyphs(regular, .horizontal, &glyph, &advance, 1)
        let scale = UIScreen.main.scale
        // The font's exact advance: runs start at `col × cellWidth` while
        // their glyphs advance by the font's own width, so a rounded cell
        // drifted every colored run off its column.
        cellWidth = advance.width
        let natural = CTFontGetAscent(regular) + CTFontGetDescent(regular)
        lineHeight = ((natural * 1.22) * scale).rounded() / scale
        ascent = CTFontGetAscent(regular) + (lineHeight - natural) / 2
    }
}

/// Paints one terminal frame: backgrounds, the selection wash, then text.
final class TerminalCanvas: UIView {
    var frameModel: TerminalFrame?
    var metrics = TerminalMetrics(size: 12)
    /// 1 at the moment of a copy (a brighter selection wash).
    var selectionBoost: CGFloat = 0

    override init(frame: CGRect) {
        super.init(frame: frame)
        isOpaque = false
        contentMode = .topLeft
    }

    required init?(coder: NSCoder) { fatalError() }

    private static func color(_ rgba: UInt32) -> UIColor {
        UIColor(
            red: CGFloat((rgba >> 24) & 0xFF) / 255,
            green: CGFloat((rgba >> 16) & 0xFF) / 255,
            blue: CGFloat((rgba >> 8) & 0xFF) / 255,
            alpha: CGFloat(rgba & 0xFF) / 255
        )
    }

    /// The scalar `TerminalGlyphs` paints for `ch`, if it does.
    private static func drawn(_ ch: Character) -> Unicode.Scalar? {
        guard ch.unicodeScalars.count == 1, let scalar = ch.unicodeScalars.first, TerminalGlyphs.draws(scalar) else { return nil }
        return scalar
    }

    override func draw(_ rect: CGRect) {
        guard let frame = frameModel, let ctx = UIGraphicsGetCurrentContext() else { return }
        let m = metrics
        let inset = TerminalView.inset
        let fg = Palette.text.resolvedColor(with: traitCollection)
        let selection = Palette.accent.resolvedColor(with: traitCollection).withAlphaComponent(0.28 + 0.3 * selectionBoost)
        // A partial scroll: rows sit `dy` lower, the revealed row above them.
        let dy = CGFloat(frame.scrollFrac) * m.lineHeight
        let rows: [(Int, TerminalLine)] = (frame.above.map { [(-1, $0)] } ?? []) + frame.lines.enumerated().map { ($0.offset, $0.element) }
        for (r, line) in rows {
            let y = inset.top + CGFloat(r) * m.lineHeight + dy
            guard y < rect.maxY, y + m.lineHeight > rect.minY else { continue }
            for run in line.runs {
                let x = inset.left + CGFloat(run.col) * m.cellWidth
                let w = CGFloat(run.cells) * m.cellWidth
                if run.bg != 0 {
                    ctx.setFillColor(Self.color(run.bg).cgColor)
                    ctx.fill(CGRect(x: x, y: y, width: w, height: m.lineHeight))
                }
                if run.selected {
                    ctx.setFillColor(selection.cgColor)
                    ctx.fill(CGRect(x: x, y: y, width: w, height: m.lineHeight))
                }
            }
        }
        // Box drawing, blocks and the bullet: painted into their cells.
        let scale = traitCollection.displayScale
        let xHeight = CTFontGetXHeight(m.regular)
        for (r, line) in rows {
            let y = inset.top + CGFloat(r) * m.lineHeight + dy
            guard y < rect.maxY, y + m.lineHeight > rect.minY else { continue }
            for run in line.runs where !run.text.allSatisfy(\.isASCII) {
                let color = run.fg == 0 ? fg : Self.color(run.fg)
                let span = (run.wide ? 2 : 1) * m.cellWidth
                var x = inset.left + CGFloat(run.col) * m.cellWidth
                for ch in run.text {
                    if let scalar = Self.drawn(ch) {
                        let cell = CGRect(x: x, y: y, width: span, height: m.lineHeight)
                        TerminalGlyphs.draw(scalar, in: cell, baseline: y + m.ascent, xHeight: xHeight, color: color, ctx: ctx, scale: scale)
                    }
                    x += span
                }
            }
        }
        ctx.textMatrix = .identity
        ctx.translateBy(x: 0, y: bounds.height)
        ctx.scaleBy(x: 1, y: -1)
        for (r, line) in rows {
            let top = inset.top + CGFloat(r) * m.lineHeight + dy
            guard top < rect.maxY, top + m.lineHeight > rect.minY else { continue }
            let baseline = bounds.height - (top + m.ascent)
            for run in line.runs where !run.text.trimmingCharacters(in: .whitespaces).isEmpty {
                let font = run.bold ? m.bold : run.italic ? m.italic : m.regular
                let color = run.fg == 0 ? fg : Self.color(run.fg)
                var attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color]
                if run.underline { attrs[.underlineStyle] = NSUnderlineStyle.single.rawValue }
                // Typed ahead of the host's echo: a faint underline owns that
                // it's provisional.
                if run.predicted {
                    attrs[.underlineStyle] = NSUnderlineStyle.single.rawValue
                    attrs[.underlineColor] = color.withAlphaComponent(0.45)
                }
                let x = inset.left + CGFloat(run.col) * m.cellWidth
                if run.text.allSatisfy(\.isASCII) {
                    let line = CTLineCreateWithAttributedString(NSAttributedString(string: run.text, attributes: attrs))
                    ctx.textPosition = CGPoint(x: x, y: baseline)
                    CTLineDraw(line, ctx)
                } else {
                    // Fallback glyphs (CJK, symbols, emoji) don't share the
                    // mono advance: place each on its own cell(s), centered.
                    var col = x
                    for ch in run.text {
                        let span = (run.wide ? 2 : 1) * m.cellWidth
                        if Self.drawn(ch) != nil {
                            col += span
                            continue
                        }
                        // A one-cell symbol that also has an emoji form (`⏵`,
                        // `☐`): ask for the text glyph, not the color emoji.
                        let emojiCapable = ch.unicodeScalars.count == 1 && ch.unicodeScalars.first!.properties.isEmoji
                        let glyph = !run.wide && emojiCapable ? "\(ch)\u{FE0E}" : String(ch)
                        let line = CTLineCreateWithAttributedString(NSAttributedString(string: glyph, attributes: attrs))
                        let width = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
                        ctx.textPosition = CGPoint(x: col + max(0, (span - width) / 2), y: baseline)
                        CTLineDraw(line, ctx)
                        col += span
                    }
                }
            }
        }
    }
}
