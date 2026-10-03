import UIKit
import QuickLook
import UniformTypeIdentifiers

/// Preview copies never expose the shell bridge to document content.
final class NativeWorkspacePreviewController: UIViewController, QLPreviewControllerDataSource {
    private let path: String
    private let shell: MobileShellRuntime
    private let exporter = NativeWorkspaceExporter()
    private var previewURL: URL?
    private var staging: URL?
    private var text: String?
    private var preview: UIViewController?
    init(path: String, shell: MobileShellRuntime) {
        self.path = path; self.shell = shell
        super.init(nibName: nil, bundle: nil)
    }
    required init?(coder: NSCoder) { fatalError() }
    deinit { if let staging { try? FileManager.default.removeItem(at: staging) } }
    override func viewDidLoad() {
        super.viewDidLoad()
        title = (path as NSString).lastPathComponent
        view.backgroundColor = .systemBackground
        let save = UIBarButtonItem(title: "Save to Files", image: UIImage(systemName: "square.and.arrow.down"), primaryAction: UIAction { [weak self] _ in
            guard let self else { return }
            self.exporter.save(shell: self.shell, path: self.path, from: self)
        })
        save.accessibilityIdentifier = "workspace-save-file"
        navigationItem.rightBarButtonItem = save
    }
    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        Task {
            do {
                guard let entry = try await shell.entries().first(where: { $0.path == path }) else { throw NativeWorkspaceFiles.failure("This file no longer exists.") }
                render(entry, data: try await shell.fileData(path))
            } catch { workspaceError(error) }
        }
    }
    private func render(_ entry: NativeWorkspaceEntry, data: Data) {
        preview?.willMove(toParent: nil); preview?.view.removeFromSuperview(); preview?.removeFromParent(); preview = nil
        view.subviews.forEach { $0.removeFromSuperview() }
        if let staging { try? FileManager.default.removeItem(at: staging) }
        staging = nil; previewURL = nil
        let metadata = UILabel()
        metadata.text = path + "\n" + NativeWorkspacePresentation.detail(entry)
        metadata.numberOfLines = 0
        metadata.font = .preferredFont(forTextStyle: .caption1)
        metadata.textColor = .secondaryLabel
        metadata.adjustsFontForContentSizeCategory = true
        metadata.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(metadata)
        let content = UIView()
        content.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(content)
        NSLayoutConstraint.activate([
            metadata.leadingAnchor.constraint(equalTo: view.layoutMarginsGuide.leadingAnchor), metadata.trailingAnchor.constraint(equalTo: view.layoutMarginsGuide.trailingAnchor),
            metadata.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 12),
            content.topAnchor.constraint(equalTo: metadata.bottomAnchor, constant: 12), content.leadingAnchor.constraint(equalTo: view.leadingAnchor), content.trailingAnchor.constraint(equalTo: view.trailingAnchor), content.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor)
        ])
        text = String(data: data, encoding: .utf8)
        let documentPreview = NativeWorkspacePresentation.type(entry).map { $0.conforms(to: .image) || $0.conforms(to: .pdf) || $0.conforms(to: .audio) || $0.conforms(to: .movie) } ?? false
        if let text, !text.contains("\0"), !documentPreview {
            let reader = UITextView()
            reader.text = text
            reader.accessibilityIdentifier = "workspace-file-content"
            reader.isEditable = false
            reader.font = UIFontMetrics(forTextStyle: .body).scaledFont(for: .monospacedSystemFont(ofSize: 14, weight: .regular))
            reader.adjustsFontForContentSizeCategory = true
            reader.textContainerInset = UIEdgeInsets(top: 16, left: 16, bottom: 24, right: 16)
            reader.backgroundColor = .secondarySystemBackground
            reader.frame = content.bounds; reader.autoresizingMask = [.flexibleWidth, .flexibleHeight]
            content.addSubview(reader)
            let edit = UIBarButtonItem(title: "Edit", primaryAction: UIAction { [weak self] _ in
                guard let self else { return }
                self.navigationController?.pushViewController(NativeCodexFileViewController(path: self.path, text: text, shell: self.shell), animated: true)
            })
            navigationItem.rightBarButtonItems = [navigationItem.rightBarButtonItems!.first!, edit]
            if ["html", "htm"].contains((path as NSString).pathExtension.lowercased()) {
                navigationItem.rightBarButtonItems?.append(UIBarButtonItem(title: "Open website", image: UIImage(systemName: "globe"), primaryAction: UIAction { [weak self] _ in
                    guard let self else { return }; self.navigationController?.pushViewController(NativeWebsiteViewController(shell: self.shell, entry: self.path), animated: true)
                }))
            }
        } else {
            navigationItem.rightBarButtonItems = [navigationItem.rightBarButtonItems!.first!]
            do {
                let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
                staging = folder
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                let url = folder.appendingPathComponent((path as NSString).lastPathComponent)
                try data.write(to: url, options: .atomic)
                previewURL = url
                if QLPreviewController.canPreview(url as NSURL) {
                    let controller = QLPreviewController(); controller.dataSource = self
                    addChild(controller); controller.view.frame = content.bounds; controller.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
                    content.addSubview(controller.view); controller.didMove(toParent: self); preview = controller
                } else {
                    var config = UIContentUnavailableConfiguration.empty()
                    config.image = UIImage(systemName: "doc")
                    config.text = "Preview unavailable"
                    config.secondaryText = "Save this file to Files to open it in another app."
                    let empty = UIContentUnavailableView(configuration: config)
                    empty.frame = content.bounds; empty.autoresizingMask = [.flexibleWidth, .flexibleHeight]; content.addSubview(empty)
                }
            } catch { workspaceError(error) }
        }
    }
    func numberOfPreviewItems(in controller: QLPreviewController) -> Int { previewURL == nil ? 0 : 1 }
    func previewController(_ controller: QLPreviewController, previewItemAt index: Int) -> QLPreviewItem { previewURL! as NSURL }
}

@MainActor
final class NativeWorkspaceExporter: NSObject, UIDocumentPickerDelegate, UIAdaptivePresentationControllerDelegate {
    private var staging: URL?
    private var preparing = false
    func save(shell: MobileShellRuntime, path: String?, from controller: UIViewController) {
        guard !preparing, staging == nil else { return }
        preparing = true
        Task {
            defer { preparing = false }
            do {
                let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
                staging = folder
                let url = try await shell.exportSelection(path: path, to: folder)
                let picker = UIDocumentPickerViewController(forExporting: [url], asCopy: true)
                picker.delegate = self
                controller.present(picker, animated: true)
                picker.presentationController?.delegate = self
            } catch { cleanup(); controller.workspaceError(error) }
        }
    }
    private func cleanup() { if let staging { try? FileManager.default.removeItem(at: staging) }; staging = nil }
    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) { cleanup() }
    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) { cleanup() }
    func presentationControllerDidDismiss(_ presentationController: UIPresentationController) { cleanup() }
}
