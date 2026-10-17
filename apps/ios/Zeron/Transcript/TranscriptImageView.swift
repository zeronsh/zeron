import UIKit

final class TranscriptImageView: UIImageView {
    private let status = UILabel()
    private let spinner = UIActivityIndicatorView(style: .medium)
    var onActivate: (() -> Void)?

    init(label: String) {
        super.init(frame: .zero)
        contentMode = .scaleAspectFit
        clipsToBounds = true
        layer.cornerCurve = .continuous
        backgroundColor = Palette.elevated
        isUserInteractionEnabled = true
        isAccessibilityElement = true
        accessibilityTraits = .image
        accessibilityLabel = label
        status.font = Fonts.ui(.sans, 13)
        status.textColor = Palette.secondary
        status.textAlignment = .center
        status.numberOfLines = 0
        status.isAccessibilityElement = false
        spinner.color = Palette.secondary
        spinner.isAccessibilityElement = false
        for view in [status, spinner] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }
        NSLayoutConstraint.activate([
            status.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
            status.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            status.centerYAnchor.constraint(equalTo: centerYAnchor),
            spinner.centerXAnchor.constraint(equalTo: centerXAnchor),
            spinner.bottomAnchor.constraint(equalTo: status.topAnchor, constant: -8),
        ])
        showLoading()
    }

    required init?(coder: NSCoder) { fatalError() }

    func showLoading() {
        image = nil
        status.text = "Loading image"
        status.isHidden = false
        spinner.startAnimating()
        accessibilityValue = "Loading image"
        accessibilityHint = nil
    }

    func showImage(_ image: UIImage) {
        self.image = image
        status.isHidden = true
        spinner.stopAnimating()
        accessibilityValue = "Image loaded"
        accessibilityHint = "Open image preview"
    }

    func showFailure() {
        image = nil
        status.text = "Image unavailable\nCheck the host connection. Use an image in the project folder.\nTap to retry."
        status.isHidden = false
        spinner.stopAnimating()
        accessibilityValue = status.text
        accessibilityHint = "Tap to retry."
    }

    override func accessibilityActivate() -> Bool {
        guard let onActivate else { return false }
        onActivate()
        return true
    }
}
