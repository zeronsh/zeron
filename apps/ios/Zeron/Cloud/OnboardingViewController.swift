import UIKit

/// First run, in the desktop's steps: make it yours (a look and a new-chat
/// wallpaper), sync (log in), connect GitHub, your agents (Settings →
/// Providers itself, with its device switcher), then notifications — the
/// phone's own — and "You're all set". The first two run signed out from the
/// welcome page; the rest pick up after logging in. Everything can be
/// changed later in Settings.
final class OnboardingViewController: UIViewController {
    enum Step: Int, CaseIterable {
        case appearance, sync, github, agents, notifications, done

        /// "2 of 5" (Done has no number).
        var number: Int? { self == .done ? nil : rawValue + 1 }
        static let total = 5

        var previous: Step? { Step(rawValue: rawValue - 1) }
        var next: Step { Step(rawValue: rawValue + 1) ?? .done }

        init?(name: String) {
            guard let step = Step.allCases.first(where: { "\($0)" == name }) else { return nil }
            self = step
        }
    }

    static let appearanceSeenKey = "onboarding.appearanceSeen"

    private let app: AppModel
    private var step: Step
    private let workos = WorkOSSignIn()
    private var loggingIn = false
    private var loginError: String?
    private var cloudToken: AnyObject?
    private var wallpaperObserver: NSObjectProtocol?
    /// Called when the flow ends; `startSession` asks for the new-session page.
    var onFinish: ((_ startSession: Bool) -> Void)?

    private let wallpaper = WallpaperView()
    private let progress = UIStackView()
    private let page = UIView()
    private let footer = UIStackView()
    private var providers: ProvidersViewController?

    static func shouldShow(for app: AppModel) -> Bool {
        guard let client = app.client, !app.isDemo else { return false }
        return !UserDefaults.standard.bool(forKey: key(client))
    }

    static func markDone(for app: AppModel) {
        guard let client = app.client else { return }
        UserDefaults.standard.set(true, forKey: key(client))
    }

    private static func key(_ client: CoreClient) -> String {
        "onboarded:\(client.userId())"
    }

    init(app: AppModel, step: Step = .github) {
        self.app = app
        self.step = step
        super.init(nibName: nil, bundle: nil)
        modalPresentationStyle = .fullScreen
    }

    required init?(coder: NSCoder) { fatalError() }

    deinit { wallpaperObserver.map(NotificationCenter.default.removeObserver) }

    @MainActor private var cloud: CloudStore { app.cloud }

    /// The edge has answered that this account has no Cloud: its steps
    /// (GitHub) and lines (Cloud agents) are left out. Unknown counts as
    /// offered until the status arrives.
    @MainActor private var cloudWithheld: Bool { cloud.info != nil && !cloud.available }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        // The new-chat wallpaper behind every step, so picking one shows it.
        wallpaper.frame = view.bounds
        wallpaper.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(wallpaper)
        // The page runs edge to edge on a phone, so it can't sit in a clear
        // patch of the wallpaper as the desktop's column does: blur it to its
        // colours instead, so text over it stays legible. The appearance step
        // previews it sharp.
        wallpaper.softened = true
        progress.axis = .horizontal
        progress.spacing = 4
        progress.alignment = .center
        progress.translatesAutoresizingMaskIntoConstraints = false
        page.translatesAutoresizingMaskIntoConstraints = false
        footer.axis = .horizontal
        footer.spacing = 12
        footer.alignment = .center
        footer.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(progress)
        view.addSubview(page)
        view.addSubview(footer)
        let guide = view.safeAreaLayoutGuide
        let width = page.widthAnchor.constraint(equalToConstant: 560)
        width.priority = .defaultHigh
        NSLayoutConstraint.activate([
            progress.topAnchor.constraint(equalTo: guide.topAnchor, constant: 14),
            progress.leadingAnchor.constraint(equalTo: page.leadingAnchor),
            progress.heightAnchor.constraint(equalToConstant: 20),
            page.topAnchor.constraint(equalTo: progress.bottomAnchor, constant: 18),
            page.centerXAnchor.constraint(equalTo: guide.centerXAnchor),
            page.leadingAnchor.constraint(greaterThanOrEqualTo: guide.leadingAnchor, constant: 24),
            page.trailingAnchor.constraint(lessThanOrEqualTo: guide.trailingAnchor, constant: -24),
            width,
            page.bottomAnchor.constraint(equalTo: footer.topAnchor, constant: -12),
            footer.leadingAnchor.constraint(equalTo: page.leadingAnchor),
            footer.trailingAnchor.constraint(equalTo: page.trailingAnchor),
            footer.bottomAnchor.constraint(equalTo: guide.bottomAnchor, constant: -12),
        ])
        if app.isSignedIn {
            cloudToken = cloud.observe { [weak self] in self?.refresh() }
            cloud.refresh()
        }
        wallpaperObserver = NotificationCenter.default.addObserver(forName: WallpaperStore.didChange, object: nil, queue: .main) { [weak self] _ in
            if self?.step == .appearance { self?.render(animated: false) }
        }
        render(animated: false)
    }

    // MARK: Pages

    /// Re-reads live state into the page in place (sign-ins, GitHub).
    private var refresher: (() -> Void)?

    private func refresh() {
        // The status arrived while on a Cloud-only step: move past it.
        if step == .github, cloudWithheld { return go(.agents) }
        refresher?()
        renderFooter()
    }

    private func go(_ next: Step) {
        if step == .appearance { UserDefaults.standard.set(true, forKey: Self.appearanceSeenKey) }
        step = next
        render(animated: true)
    }

    private func render(animated: Bool) {
        if step == .github, cloudWithheld { step = .agents }
        refresher = nil
        renderProgress()
        let content: UIView
        switch step {
        case .appearance: content = scrolling(appearancePage())
        case .sync: content = scrolling(syncPage())
        case .github: content = scrolling(githubPage())
        case .agents: content = agentsPage()
        case .notifications: content = scrolling(notificationsPage())
        case .done: content = scrolling(donePage())
        }
        if step != .agents, let providers {
            providers.willMove(toParent: nil)
            providers.removeFromParent()
            self.providers = nil
        }
        renderFooter()
        content.translatesAutoresizingMaskIntoConstraints = false
        let old = page.subviews
        page.addSubview(content)
        NSLayoutConstraint.activate([
            content.topAnchor.constraint(equalTo: page.topAnchor),
            content.leadingAnchor.constraint(equalTo: page.leadingAnchor),
            content.trailingAnchor.constraint(equalTo: page.trailingAnchor),
            content.bottomAnchor.constraint(equalTo: page.bottomAnchor),
        ])
        guard animated, !UIAccessibility.isReduceMotionEnabled else {
            old.forEach { $0.removeFromSuperview() }
            return
        }
        content.alpha = 0
        content.transform = CGAffineTransform(translationX: 20, y: 0)
        UIView.animate(withDuration: 0.3, delay: 0, options: [.curveEaseOut]) {
            content.alpha = 1
            content.transform = .identity
            old.forEach { $0.alpha = 0 }
        } completion: { _ in
            old.forEach { $0.removeFromSuperview() }
        }
    }

    /// The desktop's progress: a short bar per step, then "2 of 5".
    private func renderProgress() {
        progress.arrangedSubviews.forEach { $0.removeFromSuperview() }
        guard let number = step.number else {
            progress.isHidden = true
            return
        }
        progress.isHidden = false
        for ix in 1...Step.total {
            let bar = UIView()
            bar.backgroundColor = ix <= number ? Palette.accent : Palette.controlFill
            bar.layer.cornerRadius = 1.5
            bar.translatesAutoresizingMaskIntoConstraints = false
            NSLayoutConstraint.activate([bar.widthAnchor.constraint(equalToConstant: 20), bar.heightAnchor.constraint(equalToConstant: 3)])
            progress.addArrangedSubview(bar)
        }
        let label = UILabel()
        label.text = "\(number) of \(Step.total)"
        label.font = Fonts.ui(.sans, 12)
        label.textColor = Palette.tertiary
        progress.addArrangedSubview(label)
        progress.setCustomSpacing(10, after: progress.arrangedSubviews[Step.total - 1])
    }

    /// Back on the left; Skip and the way forward on the right.
    private func renderFooter() {
        footer.arrangedSubviews.forEach { $0.removeFromSuperview() }
        guard step != .done else {
            footer.isHidden = true
            return
        }
        footer.isHidden = false
        if var previous = step.previous {
            if previous == .github, cloudWithheld { previous = .sync }
            footer.addArrangedSubview(ghost("Back") { [weak self] in self?.go(previous) })
        } else if !app.isSignedIn {
            // Back to the welcome page.
            footer.addArrangedSubview(ghost("Back") { [weak self] in self?.dismiss(animated: true) })
        }
        footer.addArrangedSubview(UIView())
        switch step {
        case .appearance:
            footer.addArrangedSubview(primary("Continue") { [weak self] in self?.go(.sync) })
        case .sync:
            if app.isSignedIn {
                footer.addArrangedSubview(primary("Continue") { [weak self] in self?.go(.github) })
            } else {
                let login = primary(loggingIn ? "Logging in…" : "Log in") { [weak self] in self?.logIn() }
                login.isEnabled = !loggingIn
                login.accessibilityIdentifier = "onboarding-log-in"
                footer.addArrangedSubview(login)
            }
        case .github:
            if cloud.githubConnected {
                footer.addArrangedSubview(primary("Continue") { [weak self] in self?.go(.agents) })
            } else {
                footer.addArrangedSubview(ghost("Skip for now") { [weak self] in self?.go(.agents) })
            }
        case .agents:
            footer.addArrangedSubview(primary("Continue") { [weak self] in self?.go(.notifications) })
        case .notifications:
            footer.addArrangedSubview(ghost("Not now") { [weak self] in self?.go(.done) })
            footer.addArrangedSubview(primary("Turn on") { [weak self] in
                Task { @MainActor in
                    PushNotifications.shared.enabled = true
                    await PushNotifications.shared.requestPermission()
                    self?.go(.done)
                }
            })
        case .done:
            break
        }
    }

    private func appearancePage() -> UIView {
        let header = heading("Make it yours", "Pick a look. You can change it any time in Settings → Appearance.")
        let style = UserDefaults.standard.integer(forKey: "appearance")
        let scheme = UISegmentedControl(items: ["System", "Light", "Dark"])
        scheme.selectedSegmentIndex = style
        scheme.accessibilityIdentifier = "onboarding-scheme"
        scheme.addAction(UIAction { [weak self, weak scheme] _ in
            let style = scheme?.selectedSegmentIndex ?? 0
            UserDefaults.standard.set(style, forKey: "appearance")
            self?.view.window?.overrideUserInterfaceStyle = UIUserInterfaceStyle(rawValue: style) ?? .unspecified
        }, for: .valueChanged)

        let set = WallpaperStore.isSet
        let image = CloudRow(tile: CloudUI.tile(image: CloudUI.symbol("photo", 17)), title: set ? (WallpaperStore.name ?? "Wallpaper") : "No wallpaper", detail: "Shows behind every new chat.")
        image.setAccessory(CloudRow.rowButton(set ? "Change" : "Choose image", prominent: !set) { [weak self] in
            guard let self else { return }
            WallpaperPicker.present(from: self)
        })
        image.accessibilityIdentifier = "onboarding-wallpaper"
        var rows: [UIView] = [image]
        if set {
            let effect = CloudRow(tile: CloudUI.tile(image: CloudUI.symbol("wand.and.stars", 17)), title: "Background effect", detail: WallpaperStore.detail(WallpaperStore.effect))
            var c = UIButton.Configuration.glass()
            c.title = WallpaperStore.label(WallpaperStore.effect)
            c.image = CloudUI.symbol("chevron.up.chevron.down", 11, weight: .semibold)
            c.imagePlacement = .trailing
            c.imagePadding = 6
            c.baseForegroundColor = Palette.text
            c.cornerStyle = .capsule
            c.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { a in
                var a = a
                a.font = Fonts.ui(.sansMedium, 14)
                return a
            }
            let menu = UIButton(configuration: c)
            menu.menu = UIMenu(children: WallpaperStore.allEffects.map { e in
                UIAction(title: WallpaperStore.label(e), state: e == WallpaperStore.effect ? .on : .off) { _ in WallpaperStore.effect = e }
            } + [UIMenu(options: .displayInline, children: [
                UIAction(title: "Remove Wallpaper", image: UIImage(systemName: "trash"), attributes: .destructive) { _ in WallpaperStore.remove() },
            ])])
            menu.showsMenuAsPrimaryAction = true
            effect.setAccessory(menu)
            rows.append(effect)
        }
        let wallpaperGroup = UIStackView(arrangedSubviews: [CloudCard(rows: rows)])
        wallpaperGroup.axis = .vertical
        wallpaperGroup.spacing = 10
        if set { wallpaperGroup.insertArrangedSubview(WallpaperPreview(), at: 0) }
        return stack([
            header,
            group("Color scheme", scheme),
            group("New chat wallpaper", wallpaperGroup),
        ])
    }

    private func syncPage() -> UIView {
        let header = heading("Sync your work", "Log in to keep your chats, projects and settings in step everywhere you use Zeron.")
        let rows = [
            ("iphone.and.arrow.forward", "On every device", "Pick up a chat on your phone or another computer, right where you left it."),
            ("laptopcomputer", "Run agents anywhere", "Start work on any of your machines from whichever one you're at."),
            ("cloud", "Zeron Cloud", "Hand chats to cloud machines that keep going while your computer sleeps."),
        ].map { symbol, title, detail in
            CloudRow(tile: CloudUI.tile(image: CloudUI.symbol(symbol, 17)), title: title, detail: detail)
        }
        let status = UILabel()
        status.font = Fonts.ui(.sans, 14)
        status.numberOfLines = 0
        if app.isSignedIn {
            status.text = AccountProfile.load().email.map { "Signed in as \($0)" } ?? "You're signed in"
            status.textColor = Palette.secondary
        } else if let loginError {
            status.text = loginError
            status.textColor = Palette.danger
        }
        return stack([header, CloudCard(rows: rows), status])
    }

    private func githubPage() -> UIView {
        let header = heading("Connect GitHub", "Cloud agents work in your repositories on machines of their own. Connect GitHub so they can clone and push — or skip and do it later in Settings → Cloud.")
        let row = CloudRow(tile: CloudUI.tile(image: CloudUI.githubMark), title: "GitHub", detail: nil)
        row.accessibilityIdentifier = "onboarding-github"
        refresher = { [weak self, weak row] in
            guard let self, let row else { return }
            if self.cloud.githubConnected {
                row.titleLabel.text = self.cloud.githubAccount.map { "@\($0)" } ?? "GitHub"
                let n = self.cloud.repos?.count ?? 0
                row.detailLabel.text = n == 1 ? "1 repository" : "\(n) repositories"
                row.setAccessory(CloudRow.check())
            } else {
                row.titleLabel.text = "GitHub"
                row.detailLabel.text = "Clone and push from Cloud"
                row.setAccessory(CloudRow.rowButton("Connect") { [weak self] in self?.connectGitHub() })
            }
        }
        refresher?()
        return stack([header, CloudCard(rows: [row])])
    }

    /// Settings → Providers itself under the heading: its device switcher,
    /// switches and accounts.
    private func agentsPage() -> UIView {
        let header = heading("Your agents", "Looking for coding agents…")
        let detail = header.arrangedSubviews.last as? UILabel
        let providers = self.providers ?? ProvidersViewController(app: app)
        providers.onSummary = { place, cloud, names in
            guard let names else {
                detail?.text = "Looking for coding agents on \(place)…"
                return
            }
            if cloud {
                detail?.text = "Cloud runs \(Self.join(names)) with your own subscriptions. Sign in to any you'd like to use."
            } else if names.isEmpty {
                detail?.text = "Zeron runs the coding agents you already use, and none are on \(place) yet."
            } else {
                detail?.text = "Found \(names.count > 3 ? "\(names.count) agents" : Self.join(names)) on \(place). Turn on the ones you'd like to use."
            }
        }
        if self.providers == nil {
            addChild(providers)
            self.providers = providers
        }
        let holder = UIView()
        header.translatesAutoresizingMaskIntoConstraints = false
        providers.view.translatesAutoresizingMaskIntoConstraints = false
        // The page's own backdrop (the wallpaper) shows through, and the list
        // fades out above the footer rather than stopping at a hard edge.
        providers.view.backgroundColor = .clear
        let list = FadingBottomView()
        list.translatesAutoresizingMaskIntoConstraints = false
        list.addSubview(providers.view)
        holder.addSubview(header)
        holder.addSubview(list)
        NSLayoutConstraint.activate([
            header.topAnchor.constraint(equalTo: holder.topAnchor),
            header.leadingAnchor.constraint(equalTo: holder.leadingAnchor),
            header.trailingAnchor.constraint(equalTo: holder.trailingAnchor),
            list.topAnchor.constraint(equalTo: header.bottomAnchor, constant: 4),
            // The list's inset-grouped margins line up with the page's.
            list.leadingAnchor.constraint(equalTo: holder.leadingAnchor, constant: -20),
            list.trailingAnchor.constraint(equalTo: holder.trailingAnchor, constant: 20),
            list.bottomAnchor.constraint(equalTo: holder.bottomAnchor),
            providers.view.topAnchor.constraint(equalTo: list.topAnchor),
            providers.view.leadingAnchor.constraint(equalTo: list.leadingAnchor),
            providers.view.trailingAnchor.constraint(equalTo: list.trailingAnchor),
            providers.view.bottomAnchor.constraint(equalTo: list.bottomAnchor),
        ])
        providers.didMove(toParent: self)
        return holder
    }

    private func notificationsPage() -> UIView {
        let header = heading("Stay in the loop", "Get a notification when an agent finishes, needs an answer, or runs into trouble.")
        let rows = [
            ("checkmark.circle", "Finished", "A session is done"),
            ("questionmark.bubble", "Needs you", "An agent is waiting on an answer"),
            ("exclamationmark.triangle", "Something went wrong", "A run failed"),
        ].map { symbol, title, detail in
            CloudRow(tile: CloudUI.tile(image: CloudUI.symbol(symbol, 18)), title: title, detail: detail)
        }
        return stack([header, CloudCard(rows: rows)])
    }

    private func donePage() -> UIView {
        let badge = UIView()
        badge.backgroundColor = Palette.accentSoft
        badge.layer.cornerRadius = 26
        let check = UIImageView(image: CloudUI.symbol("checkmark", 22, weight: .semibold))
        check.tintColor = Palette.accent
        check.translatesAutoresizingMaskIntoConstraints = false
        badge.addSubview(check)
        badge.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            badge.widthAnchor.constraint(equalToConstant: 52),
            badge.heightAnchor.constraint(equalToConstant: 52),
            check.centerXAnchor.constraint(equalTo: badge.centerXAnchor),
            check.centerYAnchor.constraint(equalTo: badge.centerYAnchor),
        ])
        let header = CloudUI.header(
            "You're all set",
            cloudWithheld ? "Pick a project and start a session on any of your computers." : "Pick a project and start a session — on any of your computers, or on Cloud.",
            alignment: .center
        )
        let summary = UIStackView()
        summary.axis = .vertical
        summary.backgroundColor = Palette.elevated
        summary.layer.cornerRadius = 20
        summary.layer.cornerCurve = .continuous
        refresher = { [weak self, weak summary] in
            guard let self, let summary else { return }
            summary.arrangedSubviews.forEach { $0.removeFromSuperview() }
            let style = ["System", "Light", "Dark"][min(2, max(0, UserDefaults.standard.integer(forKey: "appearance")))]
            let agents = CloudStore.agents.filter { self.cloud.activeAccount($0) != nil }.map(\.name)
            var lines = [
                ("Appearance", WallpaperStore.isSet ? "\(style) · Wallpaper" : style),
                ("Sync", AccountProfile.load().email.map { "Signed in as \($0)" } ?? "Signed in"),
            ]
            if !self.cloudWithheld {
                lines += [
                    ("GitHub", self.cloud.githubConnected ? "Connected" : "Not yet · Settings → Cloud"),
                    ("Cloud agents", agents.isEmpty ? "Not yet · Settings → Providers" : Self.join(agents)),
                ]
            }
            for (ix, (label, value)) in lines.enumerated() {
                if ix > 0 {
                    let line = UIView()
                    line.backgroundColor = Palette.hairline
                    line.heightAnchor.constraint(equalToConstant: 1 / UIScreen.main.scale).isActive = true
                    summary.addArrangedSubview(self.inset(line, top: 0, bottom: 0))
                }
                let l = UILabel()
                l.text = label
                l.font = Fonts.ui(.sans, 15)
                l.textColor = Palette.secondary
                l.setContentHuggingPriority(.required, for: .horizontal)
                let v = UILabel()
                v.text = value
                v.font = Fonts.ui(.sans, 15)
                v.textColor = Palette.text
                v.textAlignment = .right
                v.lineBreakMode = .byTruncatingTail
                let row = UIStackView(arrangedSubviews: [l, v])
                row.spacing = 12
                summary.addArrangedSubview(self.inset(row, top: 13, bottom: 13))
            }
        }
        refresher?()
        let start = CloudUI.primaryButton("Start a session", symbol: "plus") { [weak self] in self?.finish(startSession: true) }
        start.accessibilityIdentifier = "onboarding-finish"
        let later = CloudUI.plainButton("I'll do this later") { [weak self] in self?.finish(startSession: false) }
        let buttons = UIStackView(arrangedSubviews: [start, later])
        buttons.axis = .vertical
        buttons.spacing = 8
        let top = UIStackView(arrangedSubviews: [badge, header])
        top.axis = .vertical
        top.alignment = .center
        top.spacing = 18
        let column = stack([spacer(24), top, summary, buttons])
        column.setCustomSpacing(0, after: column.arrangedSubviews[0])
        return column
    }

    // MARK: Pieces

    private func heading(_ title: String, _ detail: String) -> UIStackView {
        let t = UILabel()
        t.text = title
        t.font = Fonts.ui(.sansSemibold, 26)
        t.textColor = Palette.text
        t.numberOfLines = 0
        let d = UILabel()
        d.text = detail
        d.font = Fonts.ui(.sans, 15)
        d.textColor = Palette.secondary
        d.numberOfLines = 0
        let s = UIStackView(arrangedSubviews: [t, d])
        s.axis = .vertical
        s.spacing = 8
        return s
    }

    /// A small muted label over its control.
    private func group(_ title: String, _ content: UIView) -> UIView {
        let label = UILabel()
        label.text = title
        label.font = Fonts.ui(.sansMedium, 13)
        label.textColor = Palette.secondary
        let s = UIStackView(arrangedSubviews: [inset(label, top: 0, bottom: 0, side: 4), content])
        s.axis = .vertical
        s.spacing = 8
        return s
    }

    private func stack(_ views: [UIView]) -> UIStackView {
        let s = UIStackView(arrangedSubviews: views)
        s.axis = .vertical
        s.spacing = 28
        return s
    }

    private func scrolling(_ content: UIView) -> UIView {
        let scroll = UIScrollView()
        scroll.alwaysBounceVertical = false
        scroll.showsVerticalScrollIndicator = false
        content.translatesAutoresizingMaskIntoConstraints = false
        scroll.addSubview(content)
        NSLayoutConstraint.activate([
            content.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor, constant: 6),
            content.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor, constant: -16),
            content.leadingAnchor.constraint(equalTo: scroll.frameLayoutGuide.leadingAnchor),
            content.trailingAnchor.constraint(equalTo: scroll.frameLayoutGuide.trailingAnchor),
        ])
        return scroll
    }

    private func inset(_ view: UIView, top: CGFloat, bottom: CGFloat, side: CGFloat = 16) -> UIView {
        let holder = UIView()
        view.translatesAutoresizingMaskIntoConstraints = false
        holder.addSubview(view)
        NSLayoutConstraint.activate([
            view.topAnchor.constraint(equalTo: holder.topAnchor, constant: top),
            view.bottomAnchor.constraint(equalTo: holder.bottomAnchor, constant: -bottom),
            view.leadingAnchor.constraint(equalTo: holder.leadingAnchor, constant: side),
            view.trailingAnchor.constraint(equalTo: holder.trailingAnchor, constant: -side),
        ])
        return holder
    }

    private func spacer(_ height: CGFloat) -> UIView {
        let v = UIView()
        v.heightAnchor.constraint(equalToConstant: height).isActive = true
        return v
    }

    private func primary(_ title: String, action: @escaping () -> Void) -> UIButton {
        var c = UIButton.Configuration.filled()
        c.title = title
        c.baseBackgroundColor = Palette.text
        c.baseForegroundColor = Palette.background
        c.cornerStyle = .capsule
        c.contentInsets = NSDirectionalEdgeInsets(top: 12, leading: 22, bottom: 12, trailing: 22)
        c.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { a in
            var a = a
            a.font = Fonts.ui(.sansSemibold, 16)
            return a
        }
        let b = UIButton(configuration: c, primaryAction: UIAction { _ in action() })
        b.accessibilityIdentifier = b.accessibilityIdentifier ?? "onboarding-continue"
        return b
    }

    private func ghost(_ title: String, action: @escaping () -> Void) -> UIButton {
        var c = UIButton.Configuration.plain()
        c.title = title
        c.baseForegroundColor = Palette.secondary
        c.contentInsets = NSDirectionalEdgeInsets(top: 12, leading: 8, bottom: 12, trailing: 8)
        c.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { a in
            var a = a
            a.font = Fonts.ui(.sansMedium, 16)
            return a
        }
        return UIButton(configuration: c, primaryAction: UIAction { _ in action() })
    }

    /// "Claude Code", "Claude Code and Codex", "A, B and C".
    static func join(_ names: [String]) -> String {
        switch names.count {
        case 0: return ""
        case 1: return names[0]
        default: return names.dropLast().joined(separator: ", ") + " and " + names.last!
        }
    }

    // MARK: Actions

    private func logIn() {
        guard !loggingIn else { return }
        loggingIn = true
        loginError = nil
        renderFooter()
        Task { @MainActor in
            let error = await self.workos.run(app: self.app, from: self)
            // Signed in, the shell takes over and this page goes with it.
            self.loggingIn = false
            self.loginError = error
            if self.view.window != nil { self.render(animated: false) }
        }
    }

    private func connectGitHub() {
        present(GitHubConnectViewController.sheet(app: app) { [weak self] _ in self?.refresh() }, animated: true)
    }

    private func finish(startSession: Bool) {
        Self.markDone(for: app)
        let done = onFinish
        dismiss(animated: true) { done?(startSession) }
    }
}

/// The wallpaper as a new chat shows it — effect and all — since the page
/// behind onboarding only shows it blurred.
private final class WallpaperPreview: UIView {
    private let imageView = UIImageView()
    private var observer: NSObjectProtocol?

    override init(frame: CGRect) {
        super.init(frame: frame)
        backgroundColor = Palette.controlFill
        layer.cornerRadius = 18
        layer.cornerCurve = .continuous
        clipsToBounds = true
        imageView.contentMode = .scaleAspectFill
        imageView.frame = bounds
        imageView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        addSubview(imageView)
        heightAnchor.constraint(equalToConstant: 132).isActive = true
        observer = NotificationCenter.default.addObserver(forName: WallpaperStore.didChange, object: nil, queue: .main) { [weak self] _ in self?.reload() }
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (self: WallpaperPreview, _) in self.reload() }
        accessibilityIdentifier = "onboarding-wallpaper-preview"
        reload()
    }

    required init?(coder: NSCoder) { fatalError() }

    deinit { observer.map(NotificationCenter.default.removeObserver) }

    private func reload() {
        WallpaperStore.render(dark: traitCollection.userInterfaceStyle == .dark) { [weak self] render in
            self?.imageView.image = render?.image
        }
    }
}

/// Fades its content out over the bottom edge.
private final class FadingBottomView: UIView {
    private let fade = CAGradientLayer()

    override init(frame: CGRect) {
        super.init(frame: frame)
        fade.colors = [UIColor.black.cgColor, UIColor.black.cgColor, UIColor.clear.cgColor]
        layer.mask = fade
    }

    required init?(coder: NSCoder) { fatalError() }

    override func layoutSubviews() {
        super.layoutSubviews()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        fade.frame = bounds
        let edge = bounds.height > 0 ? min(0.5, 28 / bounds.height) : 0
        fade.locations = [0, NSNumber(value: Double(1 - edge)), 1]
        CATransaction.commit()
    }
}
