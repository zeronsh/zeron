import SafariServices
import UIKit

/// Settings → Billing, as on the desktop: the balance and the time it buys,
/// the three monthly plans, and extra credits that never expire. Payment
/// pages open in a browser sheet; the page re-reads when it closes.
final class BillingViewController: UIViewController, SFSafariViewControllerDelegate {
    private let app: AppModel
    private var token: AnyObject?
    private let scroll = UIScrollView()
    private let stack = UIStackView()
    private var packs = 1
    private var busy: String?

    private static let maxPacks = 20

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
        title = "Billing"
    }

    required init?(coder: NSCoder) { fatalError() }

    @MainActor private var cloud: CloudStore { app.cloud }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        navigationItem.largeTitleDisplayMode = .never
        scroll.alwaysBounceVertical = true
        scroll.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(scroll)
        stack.axis = .vertical
        stack.spacing = 8
        stack.translatesAutoresizingMaskIntoConstraints = false
        scroll.addSubview(stack)
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: view.topAnchor),
            scroll.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            stack.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor, constant: 8),
            stack.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor, constant: -32),
            stack.leadingAnchor.constraint(equalTo: scroll.frameLayoutGuide.leadingAnchor, constant: 16),
            stack.trailingAnchor.constraint(equalTo: scroll.frameLayoutGuide.trailingAnchor, constant: -16),
        ])
        token = cloud.observe { [weak self] in self?.render() }
        render()
        Task { await cloud.refreshBilling() }
    }

    // MARK: Layout

    private func render() {
        stack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        guard let billing = cloud.billing else {
            stack.addArrangedSubview(spacer(40))
            if let error = cloud.billingError {
                // A failed read is said, with a way forward — never a spinner
                // that runs forever.
                stack.addArrangedSubview(note("Couldn't load billing. \(error)"))
                let retry = CloudUI.secondaryButton("Try again", symbol: "arrow.clockwise") { [weak self] in
                    guard let self else { return }
                    Task { await self.cloud.refreshBilling() }
                }
                retry.accessibilityIdentifier = "billing-retry"
                stack.addArrangedSubview(retry)
                return
            }
            let spinner = UIActivityIndicatorView(style: .medium)
            spinner.startAnimating()
            spinner.color = Palette.secondary
            stack.addArrangedSubview(spinner)
            return
        }
        if !billing.available {
            stack.addArrangedSubview(note("Billing isn't set up on this server yet, so plans and credits can't be bought here."))
        }
        section("Credits", credits(billing))
        section("Plan", plans(billing))
        section("Extra credits", extra(billing))
    }

    private func section(_ title: String, _ content: UIView) {
        let label = UILabel()
        label.text = title
        label.font = Fonts.ui(.sansMedium, 13)
        label.textColor = Palette.secondary
        let holder = UIView()
        label.translatesAutoresizingMaskIntoConstraints = false
        holder.addSubview(label)
        NSLayoutConstraint.activate([
            label.topAnchor.constraint(equalTo: holder.topAnchor, constant: 20),
            label.bottomAnchor.constraint(equalTo: holder.bottomAnchor, constant: -2),
            label.leadingAnchor.constraint(equalTo: holder.leadingAnchor, constant: 4),
            label.trailingAnchor.constraint(equalTo: holder.trailingAnchor),
        ])
        stack.addArrangedSubview(holder)
        stack.addArrangedSubview(content)
    }

    /// The balance: one number and the time it buys, a quiet bar split into
    /// plan and extra credits, then a row for each kind.
    private func credits(_ b: BillingInfo) -> UIView {
        let total = max(b.creditsTotal, 0)
        let monthly = max(b.creditsMonthly, 0)
        let extra = max(b.creditsExtra, 0)
        let figure = FigureRow(figure: CloudUI.credits(total), unit: total == 1 ? "credit" : "credits", size: 30)
        figure.trailing = total > 0 ? Self.hours(total) : "Choose a plan or buy credits to run Cloud"
        let summary = UIStackView(arrangedSubviews: [figure])
        summary.axis = .vertical
        summary.spacing = 14
        if monthly + extra > 0 {
            summary.addArrangedSubview(SplitBar(parts: [(monthly, 0.7), (extra, 0.3)]))
        }
        let plan: (String, String)
        if b.creditsMonthlyTotal > 0 {
            let of = "Of \(CloudUI.credits(b.creditsMonthlyTotal))"
            plan = (b.creditsMonthlyEndsMs.map { "\(of) · resets \(CloudUI.shortDate($0))" } ?? of, CloudUI.credits(monthly))
        } else {
            plan = ("No plan yet — choose one below", "—")
        }
        return card([
            padded(summary, vertical: 16),
            line("Plan credits", plan.0, value: plan.1),
            line("Extra credits", "Never expire · used once plan credits run out", value: CloudUI.credits(extra)),
        ])
    }

    /// The three plans, the current one marked; on a plan, switching and
    /// canceling go through Stripe's portal.
    private func plans(_ b: BillingInfo) -> UIView {
        let current = b.plan?.id
        let idle = busy == nil && b.available
        let column = UIStackView()
        column.axis = .vertical
        column.spacing = 10
        for (ix, plan) in b.plans.enumerated() {
            let isCurrent = plan.id == current
            let action: UIButton
            if isCurrent {
                action = Self.button("Current plan", tone: .filled, enabled: false) {}
            } else if current != nil {
                action = Self.button(busy == "plan:\(plan.id)" ? "Opening…" : "Switch", tone: .filled, enabled: idle) { [weak self] in
                    self?.portal(busy: "plan:\(plan.id)")
                }
            } else {
                action = Self.button(busy == "plan:\(plan.id)" ? "Opening…" : "Choose \(plan.name)", tone: ix == 0 ? .solid : .filled, enabled: idle) { [weak self] in
                    self?.checkout(kind: "subscription", plan: plan.id, busy: "plan:\(plan.id)")
                }
            }
            action.accessibilityIdentifier = "plan-\(plan.id)"
            column.addArrangedSubview(PlanCard(plan: plan, current: isCurrent, hours: Self.hours(plan.monthlyCredits), action: action))
        }
        if let plan = b.plan {
            let detail: String
            var tint = Palette.secondary
            if b.planStatus == "past_due" {
                detail = "Your last payment didn't go through — update your card to keep your plan."
                tint = Palette.warning
            } else if b.cancelAtPeriodEnd {
                detail = b.periodEndMs.map { "Ends \(CloudUI.shortDate($0)). You keep its credits until then." } ?? "Won't renew."
            } else {
                detail = b.periodEndMs.map { "Renews \(CloudUI.shortDate($0))." } ?? "Renews monthly."
            }
            let manage = Self.button(busy == "portal" ? "Opening…" : "Manage", tone: .quiet, enabled: busy == nil) { [weak self] in
                self?.portal(busy: "portal")
            }
            manage.accessibilityIdentifier = "billing-manage"
            let row = line("\(plan.name) · \(CloudUI.price(plan.priceCents)) a month", detail, accessory: manage)
            (row.viewWithTag(Self.captionTag) as? UILabel)?.textColor = tint
            column.addArrangedSubview(card([row]))
        }
        return column
    }

    /// One-off packs: how many in one pill (− count +), then Buy.
    private func extra(_ b: BillingInfo) -> UIView {
        let credits = b.packCredits * Double(packs)
        let stepper = PackStepper(count: packs, max: Self.maxPacks) { [weak self] count in
            self?.packs = count
            self?.render()
        }
        let row = line(CloudUI.credits(credits) + " credits", "Never expire · \(Self.hours(credits))", accessory: stepper)
        let idle = busy == nil && b.available
        let buy = Self.button(busy == "credits" ? "Opening…" : "Buy for \(CloudUI.price(b.packPriceCents * UInt32(packs)))", tone: .solid, enabled: idle) { [weak self] in
            guard let self else { return }
            self.checkout(kind: "credits", packs: self.packs, busy: "credits")
        }
        buy.accessibilityIdentifier = "buy-credits"
        return card([row, padded(buy, vertical: 0, bottom: 14)], separators: false)
    }

    // MARK: Pieces

    private static let captionTag = 7301

    /// A titled row with a muted line under it and a value or control after.
    private func line(_ title: String, _ detail: String, value: String? = nil, accessory: UIView? = nil) -> UIView {
        let t = UILabel()
        t.text = title
        t.font = Fonts.ui(.sansMedium, 15)
        t.textColor = Palette.text
        let d = UILabel()
        d.text = detail
        d.font = Fonts.ui(.sans, 13)
        d.textColor = Palette.secondary
        d.numberOfLines = 0
        d.tag = Self.captionTag
        let text = UIStackView(arrangedSubviews: [t, d])
        text.axis = .vertical
        text.spacing = 2
        let row = UIStackView(arrangedSubviews: [text])
        row.alignment = .center
        row.spacing = 12
        if let value {
            let v = UILabel()
            v.text = value
            v.font = Fonts.ui(.sansMedium, 15).withMonospacedDigits()
            v.textColor = Palette.text
            v.setContentHuggingPriority(.required, for: .horizontal)
            v.setContentCompressionResistancePriority(.required, for: .horizontal)
            row.addArrangedSubview(v)
        }
        if let accessory {
            accessory.setContentHuggingPriority(.required, for: .horizontal)
            accessory.setContentCompressionResistancePriority(.required, for: .horizontal)
            row.addArrangedSubview(accessory)
        }
        return padded(row, vertical: 12)
    }

    private func padded(_ view: UIView, vertical: CGFloat, bottom: CGFloat? = nil) -> UIView {
        let holder = UIView()
        view.translatesAutoresizingMaskIntoConstraints = false
        holder.addSubview(view)
        NSLayoutConstraint.activate([
            view.topAnchor.constraint(equalTo: holder.topAnchor, constant: vertical),
            view.bottomAnchor.constraint(equalTo: holder.bottomAnchor, constant: -(bottom ?? vertical)),
            view.leadingAnchor.constraint(equalTo: holder.leadingAnchor, constant: 16),
            view.trailingAnchor.constraint(equalTo: holder.trailingAnchor, constant: -16),
        ])
        return holder
    }

    private func card(_ rows: [UIView], separators: Bool = true) -> UIView {
        let card = UIStackView()
        card.axis = .vertical
        card.backgroundColor = Palette.elevated
        card.layer.cornerRadius = 20
        card.layer.cornerCurve = .continuous
        card.clipsToBounds = true
        for (i, row) in rows.enumerated() {
            if i > 0, separators {
                let line = UIView()
                line.backgroundColor = Palette.hairline
                line.heightAnchor.constraint(equalToConstant: 1 / UIScreen.main.scale).isActive = true
                card.addArrangedSubview(padded(line, vertical: 0))
            }
            card.addArrangedSubview(row)
        }
        return card
    }

    private func note(_ text: String) -> UIView {
        let l = UILabel()
        l.text = text
        l.font = Fonts.ui(.sans, 14)
        l.textColor = Palette.secondary
        l.numberOfLines = 0
        return padded(l, vertical: 8)
    }

    private func spacer(_ height: CGFloat) -> UIView {
        let v = UIView()
        v.heightAnchor.constraint(equalToConstant: height).isActive = true
        return v
    }

    enum Tone { case solid, filled, quiet }

    /// The desktop's text actions: solid (text color), filled (a wash), quiet.
    static func button(_ title: String, tone: Tone, enabled: Bool, action: @escaping () -> Void) -> UIButton {
        var c = UIButton.Configuration.filled()
        c.title = title
        c.cornerStyle = .fixed
        c.background.cornerRadius = 10
        c.contentInsets = NSDirectionalEdgeInsets(top: 9, leading: 14, bottom: 9, trailing: 14)
        switch tone {
        case .solid:
            c.baseBackgroundColor = Palette.text
            c.baseForegroundColor = Palette.background
        case .filled:
            c.baseBackgroundColor = Palette.controlFill
            c.baseForegroundColor = Palette.text
        case .quiet:
            c.baseBackgroundColor = .clear
            c.baseForegroundColor = Palette.text
            c.background.strokeColor = Palette.hairline
            c.background.strokeWidth = 1
        }
        c.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { a in
            var a = a
            a.font = Fonts.ui(.sansMedium, 14)
            return a
        }
        let button = UIButton(configuration: c, primaryAction: UIAction { _ in action() })
        button.isEnabled = enabled
        // Disabled reads as dimmed, not as the system's grey.
        button.configurationUpdateHandler = { b in
            b.alpha = b.isEnabled ? 1 : 0.5
            var c = b.configuration
            switch tone {
            case .solid: c?.background.backgroundColor = Palette.text
            case .filled: c?.background.backgroundColor = Palette.controlFill
            case .quiet: c?.background.backgroundColor = .clear
            }
            b.configuration = c
        }
        return button
    }

    /// "about 750 hours" of the default machine.
    static func hours(_ credits: Double) -> String {
        "about \(CloudUI.credits((credits / 60).rounded())) hours"
    }

    // MARK: Payments

    private func checkout(kind: String, plan: String? = nil, packs: Int? = nil, busy: String) {
        open(busy) { client in try await client.billingCheckout(kind: kind, plan: plan, packs: packs.map(UInt32.init)) }
    }

    private func portal(busy: String) {
        open(busy) { client in try await client.billingPortal() }
    }

    private func open(_ key: String, _ link: @escaping (CoreClient) async throws -> String) {
        guard busy == nil, let client = app.client else { return }
        busy = key
        render()
        Task { @MainActor in
            defer {
                self.busy = nil
                self.render()
            }
            do {
                guard let url = URL(string: try await link(client)) else { return }
                let safari = SFSafariViewController(url: url)
                safari.delegate = self
                self.present(safari, animated: true)
            } catch {
                let a = UIAlertController(title: "Couldn't open payments", message: CloudStore.message(error), preferredStyle: .alert)
                a.addAction(UIAlertAction(title: "OK", style: .default))
                self.present(a, animated: true)
            }
        }
    }

    /// Back from Stripe: Checkout lands through a webhook, so re-read for a
    /// little while until the change shows.
    func safariViewControllerDidFinish(_ controller: SFSafariViewController) {
        let before = (cloud.billing?.plan?.id, cloud.billing?.creditsTotal)
        Task { @MainActor in
            for _ in 0..<6 {
                await self.cloud.refreshBilling()
                if (self.cloud.billing?.plan?.id, self.cloud.billing?.creditsTotal) != before { return }
                try? await Task.sleep(nanoseconds: 2_000_000_000)
            }
        }
    }
}

/// A large figure with its small unit after it on one baseline, and an
/// optional muted caption at the trailing edge on the same baseline.
final class FigureRow: UIView {
    private let figureLabel = UILabel()
    private let unitLabel = UILabel()
    private let trailingLabel = UILabel()

    var trailing: String? {
        get { trailingLabel.text }
        set { trailingLabel.text = newValue }
    }

    init(figure: String, unit: String, size: CGFloat) {
        super.init(frame: .zero)
        figureLabel.text = figure
        figureLabel.font = Fonts.ui(.sansSemibold, size).withMonospacedDigits()
        figureLabel.textColor = Palette.text
        figureLabel.accessibilityIdentifier = "cloud-credits"
        unitLabel.text = unit
        unitLabel.font = Fonts.ui(.sans, 13)
        unitLabel.textColor = Palette.secondary
        trailingLabel.font = Fonts.ui(.sans, 13)
        trailingLabel.textColor = Palette.secondary
        trailingLabel.textAlignment = .right
        trailingLabel.numberOfLines = 1
        trailingLabel.adjustsFontSizeToFitWidth = true
        trailingLabel.minimumScaleFactor = 0.8
        for l in [figureLabel, unitLabel, trailingLabel] {
            l.translatesAutoresizingMaskIntoConstraints = false
            addSubview(l)
        }
        figureLabel.setContentHuggingPriority(.required, for: .horizontal)
        unitLabel.setContentHuggingPriority(.required, for: .horizontal)
        figureLabel.setContentCompressionResistancePriority(.required, for: .horizontal)
        unitLabel.setContentCompressionResistancePriority(.required, for: .horizontal)
        NSLayoutConstraint.activate([
            figureLabel.topAnchor.constraint(equalTo: topAnchor),
            figureLabel.bottomAnchor.constraint(equalTo: bottomAnchor),
            figureLabel.leadingAnchor.constraint(equalTo: leadingAnchor),
            unitLabel.leadingAnchor.constraint(equalTo: figureLabel.trailingAnchor, constant: 6),
            unitLabel.firstBaselineAnchor.constraint(equalTo: figureLabel.firstBaselineAnchor),
            trailingLabel.leadingAnchor.constraint(greaterThanOrEqualTo: unitLabel.trailingAnchor, constant: 12),
            trailingLabel.trailingAnchor.constraint(equalTo: trailingAnchor),
            trailingLabel.firstBaselineAnchor.constraint(equalTo: figureLabel.firstBaselineAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }
}

/// A thin track split into shares, each a fainter tint of the text color.
final class SplitBar: UIView {
    init(parts: [(Double, CGFloat)]) {
        super.init(frame: .zero)
        backgroundColor = Palette.controlFill
        layer.cornerRadius = 2
        clipsToBounds = true
        heightAnchor.constraint(equalToConstant: 4).isActive = true
        let total = max(parts.map(\.0).reduce(0, +), 1)
        var previous: NSLayoutXAxisAnchor = leadingAnchor
        for (value, alpha) in parts where value > 0 {
            let segment = UIView()
            segment.backgroundColor = Palette.text.withAlphaComponent(alpha)
            segment.translatesAutoresizingMaskIntoConstraints = false
            addSubview(segment)
            NSLayoutConstraint.activate([
                segment.topAnchor.constraint(equalTo: topAnchor),
                segment.bottomAnchor.constraint(equalTo: bottomAnchor),
                segment.leadingAnchor.constraint(equalTo: previous),
                segment.widthAnchor.constraint(equalTo: widthAnchor, multiplier: CGFloat(value / total)),
            ])
            previous = segment.trailingAnchor
        }
    }

    required init?(coder: NSCoder) { fatalError() }
}

/// A plan: its name (and "Current"), the price on a baseline with "/ month",
/// the credits it brings and the time they buy, then its one action.
final class PlanCard: UIView {
    init(plan: BillingPlanInfo, current: Bool, hours: String, action: UIButton) {
        super.init(frame: .zero)
        backgroundColor = Palette.elevated
        layer.cornerRadius = 20
        layer.cornerCurve = .continuous
        layer.borderWidth = current ? 1 : 0
        layer.borderColor = Palette.text.withAlphaComponent(0.25).cgColor
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (card: PlanCard, _) in
            card.layer.borderColor = Palette.text.withAlphaComponent(0.25).resolvedColor(with: card.traitCollection).cgColor
        }
        let name = UILabel()
        name.text = plan.name
        name.font = Fonts.ui(.sansMedium, 14)
        name.textColor = Palette.secondary
        let top = UIStackView(arrangedSubviews: [name, UIView()])
        if current {
            let badge = PaddedLabel(insets: UIEdgeInsets(top: 2, left: 7, bottom: 2, right: 7))
            badge.text = "Current"
            badge.font = Fonts.ui(.sansMedium, 11)
            badge.textColor = Palette.secondary
            badge.backgroundColor = Palette.controlFill
            badge.layer.cornerRadius = 6
            badge.clipsToBounds = true
            top.addArrangedSubview(badge)
        }
        top.alignment = .center
        let price = FigureRow(figure: CloudUI.price(plan.priceCents), unit: "/ month", size: 24)
        let credits = UILabel()
        credits.text = "\(CloudUI.credits(plan.monthlyCredits)) credits a month"
        credits.font = Fonts.ui(.sans, 14)
        credits.textColor = Palette.text
        let time = UILabel()
        time.text = hours
        time.font = Fonts.ui(.sans, 13)
        time.textColor = Palette.secondary
        let text = UIStackView(arrangedSubviews: [top, price, credits, time])
        text.axis = .vertical
        text.spacing = 4
        text.setCustomSpacing(8, after: top)
        text.setCustomSpacing(8, after: price)
        let column = UIStackView(arrangedSubviews: [text, action])
        column.axis = .vertical
        column.spacing = 14
        column.translatesAutoresizingMaskIntoConstraints = false
        addSubview(column)
        NSLayoutConstraint.activate([
            column.topAnchor.constraint(equalTo: topAnchor, constant: 16),
            column.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -16),
            column.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 16),
            column.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -16),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }
}

/// − count + in one quiet pill.
final class PackStepper: UIView {
    init(count: Int, max: Int, onChange: @escaping (Int) -> Void) {
        super.init(frame: .zero)
        backgroundColor = Palette.controlFill
        layer.cornerRadius = 10
        layer.cornerCurve = .continuous
        func step(_ symbol: String, _ delta: Int, enabled: Bool, label: String) -> UIButton {
            var c = UIButton.Configuration.plain()
            c.image = CloudUI.symbol(symbol, 12, weight: .semibold)
            c.baseForegroundColor = Palette.secondary
            c.contentInsets = NSDirectionalEdgeInsets(top: 10, leading: 11, bottom: 10, trailing: 11)
            let b = UIButton(configuration: c, primaryAction: UIAction { _ in onChange(Swift.min(max, Swift.max(1, count + delta))) })
            b.isEnabled = enabled
            b.alpha = enabled ? 1 : 0.35
            b.accessibilityLabel = label
            return b
        }
        let value = UILabel()
        value.text = "\(count)"
        value.font = Fonts.ui(.sansMedium, 14).withMonospacedDigits()
        value.textColor = Palette.text
        value.textAlignment = .center
        value.widthAnchor.constraint(greaterThanOrEqualToConstant: 22).isActive = true
        let row = UIStackView(arrangedSubviews: [
            step("minus", -1, enabled: count > 1, label: "Fewer packs"),
            value,
            step("plus", 1, enabled: count < max, label: "More packs"),
        ])
        row.alignment = .center
        row.translatesAutoresizingMaskIntoConstraints = false
        addSubview(row)
        NSLayoutConstraint.activate([
            row.topAnchor.constraint(equalTo: topAnchor),
            row.bottomAnchor.constraint(equalTo: bottomAnchor),
            row.leadingAnchor.constraint(equalTo: leadingAnchor),
            row.trailingAnchor.constraint(equalTo: trailingAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }
}

/// A label with room around its text (badges).
final class PaddedLabel: UILabel {
    private let insets: UIEdgeInsets

    init(insets: UIEdgeInsets) {
        self.insets = insets
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func drawText(in rect: CGRect) {
        super.drawText(in: rect.inset(by: insets))
    }

    override var intrinsicContentSize: CGSize {
        let size = super.intrinsicContentSize
        return CGSize(width: size.width + insets.left + insets.right, height: size.height + insets.top + insets.bottom)
    }
}
