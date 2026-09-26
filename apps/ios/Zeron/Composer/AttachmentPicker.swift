import PhotosUI
import UIKit
import UniformTypeIdentifiers

/// Presents the attach menu (Photos / Camera / Files / Paste) and turns picks
/// into `StagedImage`s: HEIC→JPEG, long side ≤ 2560px, ≤ 24 MB.
final class AttachmentPicker: NSObject, PHPickerViewControllerDelegate, UIImagePickerControllerDelegate, UINavigationControllerDelegate, UIDocumentPickerDelegate {
    private weak var host: UIViewController?
    private let completion: ([StagedImage]) -> Void
    private var retainSelf: AttachmentPicker?

    init(host: UIViewController, completion: @escaping ([StagedImage]) -> Void) {
        self.host = host
        self.completion = completion
    }

    static func menu(host: UIViewController, limit: Int, completion: @escaping ([StagedImage]) -> Void) -> UIMenu {
        var items: [UIMenuElement] = [
            UIAction(title: "Photo Library", image: UIImage(systemName: "photo.on.rectangle")) { _ in
                AttachmentPicker(host: host, completion: completion).presentPhotos(limit: limit)
            },
        ]
        if UIImagePickerController.isSourceTypeAvailable(.camera) {
            items.append(UIAction(title: "Take Photo", image: UIImage(systemName: "camera")) { _ in
                AttachmentPicker(host: host, completion: completion).presentCamera()
            })
        }
        items.append(UIAction(title: "Choose File", image: UIImage(systemName: "folder")) { _ in
            AttachmentPicker(host: host, completion: completion).presentFiles()
        })
        if UIPasteboard.general.hasImages {
            items.append(UIAction(title: "Paste Image", image: UIImage(systemName: "doc.on.clipboard")) { _ in
                let staged = UIPasteboard.general.images?.compactMap { AttachmentPicker.stage(image: $0) } ?? []
                completion(staged)
            })
        }
        return UIMenu(children: items)
    }

    func presentPhotos(limit: Int) {
        var config = PHPickerConfiguration(photoLibrary: .shared())
        config.filter = .images
        config.selectionLimit = max(1, limit)
        config.preferredAssetRepresentationMode = .current
        let picker = PHPickerViewController(configuration: config)
        picker.delegate = self
        retainSelf = self
        host?.present(picker, animated: true)
    }

    func presentCamera() {
        let picker = UIImagePickerController()
        picker.sourceType = .camera
        picker.delegate = self
        retainSelf = self
        host?.present(picker, animated: true)
    }

    func presentFiles() {
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.image], asCopy: true)
        picker.allowsMultipleSelection = true
        picker.delegate = self
        retainSelf = self
        host?.present(picker, animated: true)
    }

    func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
        picker.dismiss(animated: true)
        let group = DispatchGroup()
        var staged: [(Int, StagedImage)] = []
        let lock = NSLock()
        for (i, result) in results.enumerated() {
            group.enter()
            result.itemProvider.loadDataRepresentation(forTypeIdentifier: UTType.image.identifier) { data, _ in
                defer { group.leave() }
                guard let data, let image = UIImage(data: data), let s = AttachmentPicker.stage(image: image) else { return }
                lock.lock()
                staged.append((i, s))
                lock.unlock()
            }
        }
        group.notify(queue: .main) { [self] in
            completion(staged.sorted { $0.0 < $1.0 }.map(\.1))
            retainSelf = nil
        }
    }

    func imagePickerController(_ picker: UIImagePickerController, didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]) {
        picker.dismiss(animated: true)
        if let image = info[.originalImage] as? UIImage, let s = AttachmentPicker.stage(image: image) { completion([s]) }
        retainSelf = nil
    }

    func imagePickerControllerDidCancel(_ picker: UIImagePickerController) {
        picker.dismiss(animated: true)
        retainSelf = nil
    }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        let staged = urls.compactMap { url -> StagedImage? in
            guard let data = try? Data(contentsOf: url), let image = UIImage(data: data) else { return nil }
            return AttachmentPicker.stage(image: image, name: url.deletingPathExtension().lastPathComponent)
        }
        completion(staged)
        retainSelf = nil
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) { retainSelf = nil }

    static func stage(image: UIImage, name: String? = nil) -> StagedImage? {
        let maxSide: CGFloat = 2560
        let size = image.size
        let scale = min(1, maxSide / max(size.width, size.height))
        let target = CGSize(width: (size.width * scale).rounded(), height: (size.height * scale).rounded())
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let resized = scale < 1 ? UIGraphicsImageRenderer(size: target, format: format).image { _ in image.draw(in: CGRect(origin: .zero, size: target)) } : image
        guard let data = resized.jpegData(compressionQuality: 0.86), data.count <= 24 * 1024 * 1024 else { return nil }
        let thumb = resized.preparingThumbnail(of: CGSize(width: 180, height: 180)) ?? resized
        let id = UUID().uuidString.lowercased()
        return StagedImage(id: id, name: "\(name ?? "image-\(id.prefix(8))").jpg", data: data, thumbnail: thumb)
    }
}
