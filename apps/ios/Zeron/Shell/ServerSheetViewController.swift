import SafariServices
import UIKit

/// The signed-out welcome's one way to another server: a quiet "More" in the
/// top-trailing corner, its menu "Connect to Server…" and the guide. Zeron's
/// server is the default, so the welcome names no server until someone picks
/// one (then `ServerIdentityView` does).
final class WelcomeMoreButton: UIButton {
    /// The hit target, square; the glyph sits in its middle.
    static let side: CGFloat = 44
    /// How far the glyph's edge is in from the target's: hosts set the
    /// target this far past their margin so the glyph lines up with it.
    static let glyphInset: CGFloat = 11

    init(app: AppModel, host: UIViewController) {
        super.init(frame: .zero)
        var c = UIButton.Configuration.plain()
        c.image = UIImage(systemName: "ellipsis.circle", withConfiguration: UIImage.SymbolConfiguration(pointSize: 20, weight: .regular))
        c.baseForegroundColor = Palette.secondary
        c.contentInsets = .zero
        configuration = c
        accessibilityLabel = "More"
        accessibilityIdentifier = "welcome-more"
        showsMenuAsPrimaryAction = true
        let connect = UIAction(title: "Connect to Server…", image: UIImage(systemName: "globe"), identifier: .init("connect-to-server")) { [weak host] _ in
            guard let host else { return }
            ServerSheetViewController.present(app: app, from: host)
        }
        let guide = UIAction(title: "Self-Hosting Guide", image: UIImage(systemName: "book"), identifier: .init("self-hosting-guide")) { [weak host] _ in
            guard let host else { return }
            ServerSheetViewController.openGuide(from: host)
        }
        // The action's identifier stays inside UIKit; the menu's rows carry
        // only these.
        connect.accessibilityIdentifier = "connect-to-server"
        guide.accessibilityIdentifier = "self-hosting-guide"
        menu = UIMenu(children: [connect, guide])
        translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            widthAnchor.constraint(equalToConstant: Self.side),
            heightAnchor.constraint(equalToConstant: Self.side),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }
}

/// The welcome's identity when the server in use isn't Zeron's: a capsule,
/// "[server] Acme · zeron.acme.com", at the top of the content. Tapping it
/// opens the server sheet; a server a launch argument fixed is shown, not
/// changeable. Zeron's server shows nothing, and the view hides: in a stack
/// its room closes, elsewhere it was never part of the layout.
final class ServerIdentityView: UIView {
    private let app: AppModel
    private weak var host: UIViewController?
    private let capsule = IdentityCapsule()
    private var observer: NSObjectProtocol?
    /// What the capsule shows (nil: hidden), so a change fades and a repeat
    /// of the same server doesn't.
    private var shown: ServerInfo?

    /// Off while signing in (the browser is up): the capsule stays, dimmed.
    var isEnabled = true {
        didSet { capsule.isEnabled = isEnabled }
    }

    init(app: AppModel, host: UIViewController, centered: Bool) {
        self.app = app
        self.host = host
        super.init(frame: .zero)
        capsule.addAction(UIAction { [weak self] _ in
            guard let self, let host = self.host else { return }
            ServerSheetViewController.present(app: self.app, from: host)
        }, for: .primaryActionTriggered)
        capsule.translatesAutoresizingMaskIntoConstraints = false
        addSubview(capsule)
        NSLayoutConstraint.activate([
            capsule.topAnchor.constraint(equalTo: topAnchor),
            capsule.bottomAnchor.constraint(equalTo: bottomAnchor),
            capsule.leadingAnchor.constraint(greaterThanOrEqualTo: leadingAnchor),
            capsule.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor),
            centered ? capsule.centerXAnchor.constraint(equalTo: centerXAnchor) : capsule.leadingAnchor.constraint(equalTo: leadingAnchor),
        ])
        observer = NotificationCenter.default.addObserver(forName: ChosenServer.didChange, object: nil, queue: .main) { [weak self] _ in
            self?.refresh(animated: true)
        }
        // Starts as nothing shown (`shown` is nil), so Zeron's server, the
        // usual case, leaves it hidden without a refresh doing anything.
        isHidden = true
        alpha = 0
        capsule.isHidden = true
        accessibilityElementsHidden = true
        refresh(animated: false)
    }

    required init?(coder: NSCoder) { fatalError() }

    deinit { observer.map(NotificationCenter.default.removeObserver) }

    /// Nothing while hidden. A screen that lists this view in its
    /// `accessibilityElements` reaches whatever this returns, hidden or not:
    /// left to UIKit, the empty capsule read out its icon's name.
    override var accessibilityElements: [Any]? {
        get { shown == nil ? [] : [capsule] }
        set {}
    }

    /// A 44pt-tall target around the capsule, which is shorter.
    override func hitTest(_ point: CGPoint, with event: UIEvent?) -> UIView? {
        guard !isHidden, alpha > 0.01, isUserInteractionEnabled, capsule.isTappable else { return super.hitTest(point, with: event) }
        let frame = capsule.frame
        return frame.insetBy(dx: 0, dy: min(0, (frame.height - 44) / 2)).contains(point) ? capsule : nil
    }

    /// The server in use (signed out: a launch override, else the chosen
    /// one), cross-fading when it changes.
    private func refresh(animated: Bool) {
        let server = app.server
        let info = server.isOfficial ? nil : server.info
        capsule.isTappable = SavedServer.launchOverride == nil
        guard info != shown else { return }
        let before = shown
        shown = info
        accessibilityElementsHidden = info == nil
        // The capsule hides with this view (`accessibilityElements` above is
        // what keeps VoiceOver off it): it shows before a fade in, and hides
        // once a fade out is over.
        if info != nil { capsule.isHidden = false }
        // In a stack the room opens and closes with the fade; on its own the
        // view only fades (it holds no room).
        let stacked = superview is UIStackView
        let show = {
            if let info { self.capsule.show(info) }
            if info != nil || stacked { self.isHidden = info == nil }
            self.alpha = info == nil ? 0 : 1
        }
        guard animated, window != nil, !UIAccessibility.isReduceMotionEnabled else {
            show()
            isHidden = info == nil
            capsule.isHidden = info == nil
            return
        }
        if before != nil, info != nil {
            // Another server: the words cross-fade where they stand.
            UIView.transition(with: capsule, duration: 0.25, options: [.transitionCrossDissolve, .curveEaseOut, .beginFromCurrentState, .allowUserInteraction]) {
                show()
                self.layoutIfNeeded()
            }
            return
        }
        if info != nil, isHidden {
            alpha = 0
            isHidden = false
        }
        UIView.animate(withDuration: 0.25, delay: 0, options: [.curveEaseOut, .beginFromCurrentState, .allowUserInteraction]) {
            show()
            self.window?.layoutIfNeeded()
        } completion: { _ in
            guard self.shown == nil else { return }
            self.isHidden = true
            self.capsule.isHidden = true
        }
    }
}

/// The capsule itself: the server glyph, the name in the text colour, then
/// the host quieter (left out when the name already is the host), on a soft
/// fill.
private final class IdentityCapsule: UIButton {
    private var info: ServerInfo?

    /// Off for a server a launch argument fixed: read, not pressed.
    var isTappable = true {
        didSet { isUserInteractionEnabled = isTappable }
    }

    init() {
        super.init(frame: .zero)
        var c = UIButton.Configuration.plain()
        // (The globe, as on the desktop, on Android and on the server's own page.)
        c.image = UIImage(systemName: "globe", withConfiguration: UIImage.SymbolConfiguration(pointSize: TypeScale.size(11), weight: .semibold))
        c.imagePadding = 7
        c.cornerStyle = .capsule
        c.background.backgroundColor = Palette.controlFill
        c.contentInsets = NSDirectionalEdgeInsets(top: 7, leading: 12, bottom: 7, trailing: 14)
        c.titleLineBreakMode = .byTruncatingTail
        configuration = c
        configurationUpdateHandler = { [weak self] button in self?.restyle(button) }
        accessibilityIdentifier = "server-identity"
    }

    required init?(coder: NSCoder) { fatalError() }

    override var accessibilityTraits: UIAccessibilityTraits {
        get {
            guard isTappable else { return .staticText }
            return isEnabled ? .button : [.button, .notEnabled]
        }
        set {}
    }

    func show(_ info: ServerInfo) {
        self.info = info
        let host = info.label == info.host ? nil : info.host
        accessibilityLabel = ["Server", info.label, host].compactMap { $0 }.joined(separator: ", ")
        setNeedsUpdateConfiguration()
        // Laid out now, so a cross-fade ends on the new width.
        layoutIfNeeded()
    }

    /// Dimmed pressed or disabled, as the sheet's rows.
    private func restyle(_ button: UIButton) {
        guard let info else { return }
        let dim = !button.isEnabled || button.isHighlighted
        let size = TypeScale.size(13)
        var title = AttributedString(info.label, attributes: AttributeContainer([
            .font: Fonts.ui(.sansSemibold, size),
            .foregroundColor: dim ? Palette.tertiary : Palette.text,
        ]))
        if info.label != info.host {
            title += AttributedString(" · \(info.host)", attributes: AttributeContainer([
                .font: Fonts.ui(.sans, size),
                .foregroundColor: dim ? Palette.tertiary : Palette.secondary,
            ]))
        }
        button.configuration?.attributedTitle = title
        button.configuration?.baseForegroundColor = dim ? Palette.tertiary : Palette.secondary
    }
}

/// "Choose a server": Zeron's, or one at an address someone types (the core
/// reads its discovery document). On the sign-in screen it sets the server
/// sign-in uses; from Settings (`.switchAccount`) a different server means
/// signing out first. A glass sheet sized to its contents, as the model
/// picker. An invite link (`ServerLink`) opens it already looking its
/// address up, and what it finds waits for a tap.
final class ServerSheetViewController: UIViewController, UITextFieldDelegate {
    enum Mode { case signIn, switchAccount }

    private let app: AppModel
    private let mode: Mode
    /// The server in use: the one signed in to, else sign-in's.
    private let current: SavedServer
    /// `-edge` / `-workos` decide the server: shown, not changeable.
    private let locked = SavedServer.launchOverride != nil
    private var selfHosted: Bool
    private var busy = false
    private var lookup: Task<Void, Never>?
    /// An invite's address, looked up as soon as the sheet loads.
    private var invite: String?
    /// The server an invite found, signed out: "Connect to Acme" until the
    /// address or the option changes. A link never switches on its own.
    private var found: SavedServer?
    private weak var sheet: UISheetPresentationController?

    private let header = UIView()
    private let scroll = UIScrollView()
    private let content = UIStackView()
    private let zeron = ServerOption(title: "Zeron", detail: SavedServer.official.info.host, id: "zeron")
    private let custom = ServerOption(title: "Self-hosted", detail: "Your own server", id: "self-hosted")
    private var addressBox: UIView!
    private let field = AddressField()
    private let error = UILabel()
    private var primary: UIButton!

    private static let headerTop: CGFloat = 26
    private static let contentGap: CGFloat = 16
    private static let bottomGap: CGFloat = 12

    /// The sheet for the state the app is in: choosing sign-in's server, or
    /// (signed in) switching to another account's.
    static func present(app: AppModel, from host: UIViewController) {
        host.present(sheet(app: app, mode: app.isSignedIn ? .switchAccount : .signIn), animated: true)
    }

    /// An invite's `address`, signed in or out: in the sheet already open
    /// (never a second one on top), else in a new one over whatever is
    /// frontmost.
    static func open(invite address: String, app: AppModel, over root: UIViewController) {
        var top = root
        while let next = top.presentedViewController, !next.isBeingDismissed {
            if let open = next as? ServerSheetViewController { return open.follow(invite: address) }
            top = next
        }
        let picker = sheet(app: app, mode: app.isSignedIn ? .switchAccount : .signIn, invite: address)
        // Something still sliding in (a cold launch's onboarding) or out:
        // present once it has settled.
        if let coordinator = top.transitionCoordinator ?? top.presentedViewController?.transitionCoordinator {
            coordinator.animate(alongsideTransition: nil) { [weak top] _ in top?.present(picker, animated: true) }
        } else {
            top.present(picker, animated: true)
        }
    }

    /// How to run a server of one's own, in Safari's sheet.
    static func openGuide(from host: UIViewController) {
        guard let url = URL(string: serverSelfHostingGuide()) else { return }
        host.present(SFSafariViewController(url: url), animated: true)
    }

    /// The sheet, ready to present.
    static func sheet(app: AppModel, mode: Mode, invite: String? = nil) -> UIViewController {
        let picker = ServerSheetViewController(app: app, mode: mode, invite: invite)
        picker.modalPresentationStyle = .pageSheet
        if let sheet = picker.sheetPresentationController {
            sheet.detents = [
                .custom(identifier: .init("fit")) { [weak picker] ctx in
                    picker?.fittedHeight(ctx.maximumDetentValue) ?? ctx.maximumDetentValue * 0.62
                },
                .large(),
            ]
            sheet.prefersGrabberVisible = true
            sheet.prefersScrollingExpandsWhenScrolledToEdge = true
            sheet.preferredCornerRadius = 34
            picker.sheet = sheet
        }
        return picker
    }

    private init(app: AppModel, mode: Mode, invite: String?) {
        self.app = app
        self.mode = mode
        current = mode == .switchAccount ? app.accountServer ?? app.signInServer : app.signInServer
        let shown = SavedServer.launchOverride ?? current
        selfHosted = !shown.isOfficial
        super.init(nibName: nil, bundle: nil)
        field.text = shown.isOfficial ? nil : shown.url
        // A launch argument's server stays as it is, invite or not.
        if let invite, !locked {
            self.invite = invite
            selfHosted = true
            field.text = invite
        }
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        // Clear: the sheet's own glass is the surface.
        view.backgroundColor = .clear
        buildHeader()
        buildContent()
        scroll.alwaysBounceVertical = false
        scroll.keyboardDismissMode = .onDrag
        scroll.translatesAutoresizingMaskIntoConstraints = false
        content.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(scroll)
        scroll.addSubview(content)
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: header.bottomAnchor, constant: Self.contentGap),
            scroll.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor),
            content.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor),
            content.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor, constant: -Self.bottomGap),
            content.leadingAnchor.constraint(equalTo: scroll.frameLayoutGuide.leadingAnchor, constant: 12),
            content.trailingAnchor.constraint(equalTo: scroll.frameLayoutGuide.trailingAnchor, constant: -12),
        ])
        setContentScrollView(scroll, for: .top)
        update(animated: false)
        if let invite {
            self.invite = nil
            lookUp(invite, waitForTap: true)
        }
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        refit()
    }

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        // Closed: drop the lookup (its answer would be nobody's).
        if isBeingDismissed || presentingViewController == nil { cancelLookup() }
    }

    deinit { lookup?.cancel() }

    // MARK: Building

    /// "Choose a server" and a glass close button.
    private func buildHeader() {
        let title = UILabel()
        title.text = "Choose a server"
        title.font = Fonts.ui(.sansSemibold, 22)
        title.textColor = Palette.text
        title.numberOfLines = 0
        title.accessibilityTraits = .header

        var c = UIButton.Configuration.glass()
        c.image = UIImage(systemName: "xmark", withConfiguration: UIImage.SymbolConfiguration(pointSize: 13, weight: .semibold))
        c.baseForegroundColor = Palette.secondary
        c.cornerStyle = .capsule
        let close = UIButton(configuration: c, primaryAction: UIAction { [weak self] _ in
            self?.cancelLookup()
            self?.dismiss(animated: true)
        })
        close.accessibilityLabel = "Close"
        close.accessibilityIdentifier = "server-sheet-close"

        for v in [title, close] as [UIView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            header.addSubview(v)
        }
        header.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(header)
        NSLayoutConstraint.activate([
            header.topAnchor.constraint(equalTo: view.topAnchor, constant: Self.headerTop),
            header.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 24),
            header.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -18),
            title.topAnchor.constraint(equalTo: header.topAnchor),
            title.bottomAnchor.constraint(equalTo: header.bottomAnchor),
            title.leadingAnchor.constraint(equalTo: header.leadingAnchor),
            title.trailingAnchor.constraint(lessThanOrEqualTo: close.leadingAnchor, constant: -12),
            close.trailingAnchor.constraint(equalTo: header.trailingAnchor),
            close.centerYAnchor.constraint(equalTo: title.centerYAnchor),
            close.widthAnchor.constraint(equalToConstant: 36),
            close.heightAnchor.constraint(equalToConstant: 36),
        ])
    }

    /// The intro, the two servers, the address (Self-hosted only), the way
    /// forward and the guide. Text sits 12 in from the rows' plates, level
    /// with the title.
    private func buildContent() {
        let intro = Self.label("Zeron keeps your chats in sync through a server. Use Zeron's, or one you or your team runs.", .sans, 15, Palette.secondary)

        for option in [zeron, custom] { option.isEnabled = !locked }
        zeron.addAction(UIAction { [weak self] _ in self?.choose(selfHosted: false) }, for: .touchUpInside)
        custom.addAction(UIAction { [weak self] _ in self?.choose(selfHosted: true) }, for: .touchUpInside)
        let options = UIStackView(arrangedSubviews: [zeron, custom])
        options.axis = .vertical
        options.spacing = 2

        let fieldTitle = Self.label("Server address", .sansMedium, 13, Palette.secondary)
        fieldTitle.isAccessibilityElement = false
        field.delegate = self
        field.isEnabled = !locked
        field.returnKeyType = mode == .switchAccount ? .continue : .go
        field.addAction(UIAction { [weak self] _ in self?.edited() }, for: .editingChanged)
        error.font = Fonts.ui(.sans, TypeScale.size(13))
        error.textColor = Palette.danger
        error.numberOfLines = 0
        error.isHidden = true
        error.accessibilityIdentifier = "server-error"
        let note = Self.label("Your account on a self-hosted server is separate from your Zeron account.", .sans, 13, Palette.secondary)
        let address = UIStackView(arrangedSubviews: [fieldTitle, field, error, note])
        address.axis = .vertical
        address.spacing = 8
        address.setCustomSpacing(10, after: error)
        addressBox = Self.inset(address)

        primary = CloudUI.primaryButton(primaryTitle) { [weak self] in self?.connect() }
        primary.accessibilityIdentifier = "server-connect"
        let guide = CloudUI.plainButton("Run your own server") { [weak self] in
            if let self { Self.openGuide(from: self) }
        }
        guide.accessibilityIdentifier = "server-guide"

        var items: [UIView] = [Self.inset(intro)]
        if locked { items.append(Self.inset(Self.notice("Set by a launch argument"))) }
        items += [options, addressBox, Self.inset(primary), guide]
        items.forEach(content.addArrangedSubview)
        content.axis = .vertical
        content.spacing = 20
        content.setCustomSpacing(18, after: items[0])
        content.setCustomSpacing(6, after: items[items.count - 2])
    }

    // MARK: State

    private var typed: String { field.text?.trimmingCharacters(in: .whitespacesAndNewlines) ?? "" }

    private var primaryTitle: String {
        if busy { return "Connecting…" }
        if mode == .switchAccount { return "Continue" }
        if let found { return "Connect to \(found.info.label)" }
        return selfHosted ? "Connect" : "Use Zeron"
    }

    private func choose(selfHosted: Bool) {
        guard !locked, !busy, selfHosted != self.selfHosted else { return }
        self.selfHosted = selfHosted
        found = nil
        UISelectionFeedbackGenerator().selectionChanged()
        show(error: nil)
        if !selfHosted { field.resignFirstResponder() }
        update(animated: true)
        if selfHosted { field.becomeFirstResponder() }
    }

    /// The options, the address field's reveal, and the primary button, to
    /// the current state.
    private func update(animated: Bool) {
        zeron.isSelected = !selfHosted
        custom.isSelected = selfHosted
        UIView.performWithoutAnimation {
            primary.configuration?.title = primaryTitle
            primary.layoutIfNeeded()
        }
        primary.isEnabled = !locked && !busy && (found != nil || !selfHosted || !typed.isEmpty)
        // Typing is held, not the keyboard: the sheet doesn't jump while it
        // checks.
        field.textColor = busy ? Palette.tertiary : Palette.text
        for option in [zeron, custom] { option.isEnabled = !locked && !busy }
        isModalInPresentation = busy

        guard addressBox.isHidden == selfHosted else { return }
        let reveal = {
            self.addressBox.isHidden = !self.selfHosted
            self.addressBox.alpha = self.selfHosted ? 1 : 0
        }
        guard animated, view.window != nil, !UIAccessibility.isReduceMotionEnabled else {
            reveal()
            return refit()
        }
        UIView.animate(withDuration: 0.35, delay: 0, usingSpringWithDamping: 0.9, initialSpringVelocity: 0, options: [.beginFromCurrentState, .allowUserInteraction]) {
            reveal()
            self.view.layoutIfNeeded()
        }
        refit()
    }

    private func edited() {
        found = nil
        show(error: nil)
        update(animated: false)
    }

    private func show(error message: String?) {
        guard error.text != message else { return }
        error.text = message
        error.isHidden = message == nil
        refit()
        if let message { UIAccessibility.post(notification: .announcement, argument: message) }
    }

    // MARK: Connecting

    private func connect() {
        guard !locked, !busy else { return }
        if let found { return finish(found) }
        guard selfHosted else { return finish(.official) }
        let address = typed
        guard !address.isEmpty else { return }
        lookUp(address, waitForTap: false)
    }

    /// Reads the server at `address`. Typed, what it finds is used at once;
    /// from an invite (`waitForTap`) it's only offered, signed out, as
    /// "Connect to Acme" (signed in, the switch asks first anyway).
    private func lookUp(_ address: String, waitForTap: Bool) {
        busy = true
        update(animated: false)
        lookup = Task { @MainActor [weak self] in
            let answer: Result<ServerInfo, Error>
            do { answer = .success(try await serverDiscover(address: address)) } catch { answer = .failure(error) }
            // Closed meanwhile: the answer is nobody's.
            guard let self, !Task.isCancelled else { return }
            self.lookup = nil
            self.busy = false
            switch answer {
            case let .success(info):
                let server = SavedServer(info)
                guard waitForTap, self.mode == .signIn else {
                    self.update(animated: false)
                    return self.finish(server)
                }
                // Already the one in use: nothing to ask.
                if server.isSame(as: self.current) { return self.dismiss(animated: true) }
                self.found = server
                self.update(animated: false)
                UIAccessibility.post(notification: .layoutChanged, argument: self.primary)
            case let .failure(error):
                self.update(animated: false)
                self.show(error: Self.message(for: error))
            }
        }
    }

    /// Another invite while open: its address replaces whatever was typed
    /// or found, and anything over the sheet (the guide, a confirmation)
    /// gives way first. A launch argument's server stays.
    private func follow(invite address: String) {
        guard !locked else { return }
        if let over = presentedViewController {
            guard !over.isBeingDismissed else { return }
            return over.dismiss(animated: true) { [weak self] in self?.follow(invite: address) }
        }
        cancelLookup()
        busy = false
        found = nil
        let reveal = !selfHosted
        selfHosted = true
        field.resignFirstResponder()
        field.text = address
        show(error: nil)
        update(animated: reveal)
        lookUp(address, waitForTap: true)
    }

    /// The core's sentence for why the address won't do.
    static func message(for error: Error) -> String {
        if case let ServerLookupError.Failed(_, message) = error { return message }
        return error.localizedDescription
    }

    private func cancelLookup() {
        lookup?.cancel()
        lookup = nil
    }

    private func finish(_ server: SavedServer) {
        switch mode {
        case .signIn:
            if server != current { ChosenServer.save(server) }
            dismiss(animated: true)
        case .switchAccount:
            if server.isSame(as: current) { return dismiss(animated: true) }
            confirmSwitch(to: server)
        }
    }

    /// Another server is another account: sign out here (as Settings → Sign
    /// Out does), and the sign-in screen comes back on the new server.
    private func confirmSwitch(to server: SavedServer) {
        let label = server.info.label
        let alert = UIAlertController(title: "Switch to \(label)?", message: "You'll be signed out here, then sign in on \(label). Local drafts stay on this device.", preferredStyle: .actionSheet)
        alert.addAction(UIAlertAction(title: "Sign Out and Switch", style: .destructive) { [app] _ in
            ChosenServer.save(server)
            app.signOut()
        })
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.popoverPresentationController?.sourceView = primary
        present(alert, animated: true)
    }

    // MARK: UITextFieldDelegate

    func textField(_ textField: UITextField, shouldChangeCharactersIn range: NSRange, replacementString string: String) -> Bool {
        !busy
    }

    func textFieldShouldReturn(_ textField: UITextField) -> Bool {
        if primary.isEnabled { connect() }
        return false
    }

    // MARK: Sizing

    /// Tall enough for everything, up to the large detent.
    private func fittedHeight(_ maximum: CGFloat) -> CGFloat {
        guard isViewLoaded else { return maximum * 0.62 }
        let width = view.bounds.width > 0 ? view.bounds.width : presentingViewController?.view.bounds.width ?? 0
        guard width > 0 else { return maximum * 0.62 }
        let height = Self.headerTop + Self.fitted(header, width: width - 42) + Self.contentGap + Self.fitted(content, width: width - 24) + Self.bottomGap
        return min(maximum, height.rounded(.up))
    }

    private func refit() {
        guard let sheet, view.window != nil, sheet.selectedDetentIdentifier != .large else { return }
        sheet.animateChanges { sheet.invalidateDetents() }
    }

    private static func fitted(_ view: UIView, width: CGFloat) -> CGFloat {
        view.systemLayoutSizeFitting(CGSize(width: width, height: 0), withHorizontalFittingPriority: .required, verticalFittingPriority: .fittingSizeLevel).height
    }

    // MARK: Parts

    private static func label(_ text: String, _ face: FaceRole, _ size: CGFloat, _ color: UIColor) -> UILabel {
        let label = UILabel()
        label.text = text
        label.font = Fonts.ui(face, TypeScale.size(size))
        label.textColor = color
        label.numberOfLines = 0
        return label
    }

    /// `view`, 12 in on both sides: level with the title, past the rows'
    /// plates.
    private static func inset(_ view: UIView) -> UIView {
        let box = UIView()
        view.translatesAutoresizingMaskIntoConstraints = false
        box.addSubview(view)
        NSLayoutConstraint.activate([
            view.topAnchor.constraint(equalTo: box.topAnchor),
            view.bottomAnchor.constraint(equalTo: box.bottomAnchor),
            view.leadingAnchor.constraint(equalTo: box.leadingAnchor, constant: 12),
            view.trailingAnchor.constraint(equalTo: box.trailingAnchor, constant: -12),
        ])
        return box
    }

    /// A quiet tinted line (the model picker's notice).
    private static func notice(_ text: String) -> UIView {
        let plate = UIView()
        plate.backgroundColor = Palette.controlFill
        plate.layer.cornerRadius = 14
        plate.layer.cornerCurve = .continuous
        let icon = UIImageView(image: UIImage(systemName: "info.circle", withConfiguration: UIImage.SymbolConfiguration(pointSize: 13, weight: .medium)))
        icon.tintColor = Palette.secondary
        icon.setContentHuggingPriority(.required, for: .horizontal)
        let row = UIStackView(arrangedSubviews: [icon, label(text, .sans, 13, Palette.secondary)])
        row.spacing = 8
        row.alignment = .firstBaseline
        row.translatesAutoresizingMaskIntoConstraints = false
        plate.addSubview(row)
        NSLayoutConstraint.activate([
            row.topAnchor.constraint(equalTo: plate.topAnchor, constant: 11),
            row.bottomAnchor.constraint(equalTo: plate.bottomAnchor, constant: -11),
            row.leadingAnchor.constraint(equalTo: plate.leadingAnchor, constant: 14),
            row.trailingAnchor.constraint(equalTo: plate.trailingAnchor, constant: -14),
        ])
        plate.accessibilityIdentifier = "server-locked"
        return plate
    }
}

/// One of the two servers: its name over a line of detail; the selected one
/// sits in a soft accent tint with an accent check (the model picker's rows).
private final class ServerOption: UIControl {
    private let plate = UIView()
    private let name = UILabel()
    private let detail = UILabel()
    private let check = UIImageView(image: UIImage(systemName: "checkmark", withConfiguration: UIImage.SymbolConfiguration(pointSize: 14, weight: .bold)))

    init(title: String, detail text: String, id: String) {
        super.init(frame: .zero)
        plate.layer.cornerRadius = 18
        plate.layer.cornerCurve = .continuous
        name.text = title
        name.font = Fonts.ui(.sansMedium, TypeScale.size(16))
        name.numberOfLines = 0
        detail.text = text
        detail.font = Fonts.ui(.sans, TypeScale.size(13))
        detail.textColor = Palette.secondary
        detail.numberOfLines = 0
        check.tintColor = Palette.accent
        check.contentMode = .center
        check.setContentHuggingPriority(.required, for: .horizontal)
        check.setContentCompressionResistancePriority(.required, for: .horizontal)
        let words = UIStackView(arrangedSubviews: [name, detail])
        words.axis = .vertical
        words.spacing = 3
        let row = UIStackView(arrangedSubviews: [words, check])
        row.alignment = .center
        row.spacing = 12
        for v in [plate, row] as [UIView] {
            v.isUserInteractionEnabled = false
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
        }
        NSLayoutConstraint.activate([
            plate.topAnchor.constraint(equalTo: topAnchor),
            plate.bottomAnchor.constraint(equalTo: bottomAnchor),
            plate.leadingAnchor.constraint(equalTo: leadingAnchor),
            plate.trailingAnchor.constraint(equalTo: trailingAnchor),
            row.topAnchor.constraint(equalTo: topAnchor, constant: 12),
            row.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -12),
            row.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 14),
            row.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -16),
            check.widthAnchor.constraint(equalToConstant: 20),
        ])
        isAccessibilityElement = true
        accessibilityLabel = "\(title), \(text)"
        accessibilityIdentifier = "server-option-\(id)"
        restyle()
    }

    required init?(coder: NSCoder) { fatalError() }

    override var isSelected: Bool {
        didSet { restyle() }
    }

    override var isEnabled: Bool {
        didSet { restyle() }
    }

    override var isHighlighted: Bool {
        didSet { UIView.animate(withDuration: isHighlighted ? 0.06 : 0.3, delay: 0, options: [.beginFromCurrentState, .allowUserInteraction]) { self.restyle() } }
    }

    override var accessibilityTraits: UIAccessibilityTraits {
        get {
            var traits: UIAccessibilityTraits = .button
            if isSelected { traits.insert(.selected) }
            if !isEnabled { traits.insert(.notEnabled) }
            return traits
        }
        set {}
    }

    override func accessibilityActivate() -> Bool {
        guard isEnabled else { return false }
        sendActions(for: .touchUpInside)
        return true
    }

    private func restyle() {
        plate.backgroundColor = isSelected ? Palette.accentSoft : isHighlighted ? Palette.controlFill : .clear
        plate.transform = isHighlighted ? CGAffineTransform(scaleX: 0.985, y: 0.985) : .identity
        check.alpha = isSelected ? 1 : 0
        name.textColor = isEnabled ? Palette.text : Palette.tertiary
        check.tintColor = isEnabled ? Palette.accent : Palette.tertiary
    }
}

/// The address: on a soft fill, the text inset from its rounded edge; the
/// keyboard for addresses, nothing corrected.
private final class AddressField: UITextField {
    private let padding = UIEdgeInsets(top: 0, left: 14, bottom: 0, right: 14)

    init() {
        super.init(frame: .zero)
        let size = TypeScale.size(16)
        font = Fonts.ui(.sans, size)
        textColor = Palette.text
        tintColor = Palette.accent
        backgroundColor = Palette.controlFill
        layer.cornerRadius = 14
        layer.cornerCurve = .continuous
        attributedPlaceholder = NSAttributedString(string: "zeron.example.com", attributes: [.foregroundColor: Palette.tertiary, .font: Fonts.ui(.sans, size)])
        keyboardType = .URL
        textContentType = .URL
        autocorrectionType = .no
        autocapitalizationType = .none
        spellCheckingType = .no
        smartDashesType = .no
        smartQuotesType = .no
        smartInsertDeleteType = .no
        enablesReturnKeyAutomatically = true
        accessibilityLabel = "Server address"
        accessibilityIdentifier = "server-address"
        heightAnchor.constraint(greaterThanOrEqualToConstant: max(48, size * 3)).isActive = true
    }

    required init?(coder: NSCoder) { fatalError() }

    override func textRect(forBounds bounds: CGRect) -> CGRect { bounds.inset(by: padding) }
    override func editingRect(forBounds bounds: CGRect) -> CGRect { bounds.inset(by: padding) }
    override func placeholderRect(forBounds bounds: CGRect) -> CGRect { bounds.inset(by: padding) }
}
