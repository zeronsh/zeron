import UIKit

/// Full-screen, zoomable image with drag-to-dismiss. Presented with a
/// zoom transition from the tapped thumbnail (iOS 18+ `.zoom`), so the image
/// morphs out of the transcript and back.
final class ImageViewer: UIViewController, UIScrollViewDelegate {
    private let image: UIImage
    private let scroll = UIScrollView()
    private let imageView = UIImageView()

    init(image: UIImage, source: UIView) {
        self.image = image
        super.init(nibName: nil, bundle: nil)
        preferredTransition = .zoom { [weak source] _ in source }
        modalPresentationStyle = .fullScreen
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        scroll.frame = view.bounds
        scroll.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        scroll.delegate = self
        scroll.maximumZoomScale = 5
        scroll.showsVerticalScrollIndicator = false
        scroll.showsHorizontalScrollIndicator = false
        scroll.contentInsetAdjustmentBehavior = .never
        view.addSubview(scroll)
        imageView.image = image
        imageView.contentMode = .scaleAspectFit
        scroll.addSubview(imageView)

        let close = Glass.circleButton(symbol: "xmark", size: 44, pointSize: 15, action: UIAction { [weak self] _ in self?.dismiss(animated: true) })
        close.accessibilityLabel = "Close image preview"
        let share = Glass.circleButton(symbol: "square.and.arrow.up", size: 44, pointSize: 15, action: UIAction { [weak self] _ in self?.share() })
        share.accessibilityLabel = "Share image"
        for b in [close, share] { view.addSubview(b) }
        NSLayoutConstraint.activate([
            close.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 16),
            close.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 8),
            share.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -16),
            share.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 8),
        ])
        let doubleTap = UITapGestureRecognizer(target: self, action: #selector(toggleZoom(_:)))
        doubleTap.numberOfTapsRequired = 2
        scroll.addGestureRecognizer(doubleTap)
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        guard scroll.zoomScale == 1 else { return }
        imageView.frame = scroll.bounds
        scroll.contentSize = scroll.bounds.size
    }

    func viewForZooming(in scrollView: UIScrollView) -> UIView? { imageView }

    @objc private func toggleZoom(_ tap: UITapGestureRecognizer) {
        if scroll.zoomScale > 1 {
            scroll.setZoomScale(1, animated: true)
        } else {
            let p = tap.location(in: imageView)
            let size = CGSize(width: scroll.bounds.width / 2.5, height: scroll.bounds.height / 2.5)
            scroll.zoom(to: CGRect(x: p.x - size.width / 2, y: p.y - size.height / 2, width: size.width, height: size.height), animated: true)
        }
    }

    private func share() {
        let vc = UIActivityViewController(activityItems: [image], applicationActivities: nil)
        vc.popoverPresentationController?.sourceView = view
        present(vc, animated: true)
    }
}
