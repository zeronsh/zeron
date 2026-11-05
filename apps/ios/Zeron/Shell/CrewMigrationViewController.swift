import UIKit

/// Migration runs before the live client opens. This screen only reads its receipt.
final class CrewMigrationViewController: UIViewController {
    private let app: AppModel
    private let status = UITextView()
    private var exportURLs: [URL] = []

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
        title = "Legacy iOS migration"
    }
    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        status.isEditable = false
        status.font = UIFont.preferredFont(forTextStyle: .body)
        status.text = "Sign-in does not import the legacy CometDocs cache; your sessions sync from their hosts. The cache and legacy Keychain entries stay on this phone untouched.\n\nNo legacy cache was migrated for this profile."
        status.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(status)
        NSLayoutConstraint.activate([
            status.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 16),
            status.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -16),
            status.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            status.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor),
        ])
        guard let key = app.crewStorageKey else { return }
        let profile = AppModel.profilesDir.appendingPathComponent(key, isDirectory: true)
        let receipt = profile.appendingPathComponent("legacy-ios-import.json")
        guard FileManager.default.fileExists(atPath: receipt.path) else { return }
        do {
            let data = try Data(contentsOf: receipt)
            let report = try JSONSerialization.jsonObject(with: data)
            let formatted = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
            status.text = "Native migration receipt. Any missing data or unknown command outcomes below still require reconciliation with the owning host; they were not fabricated or replayed.\n\n" + String(decoding: formatted, as: UTF8.self)
            exportURLs = [receipt, profile.appendingPathComponent("originals", isDirectory: true)]
            navigationItem.rightBarButtonItem = UIBarButtonItem(title: "Export originals", primaryAction: UIAction { [weak self] _ in
                guard let self else { return }
                // Only immutable migration input and receipt, never the live SQLite store.
                self.present(UIDocumentPickerViewController(forExporting: self.exportURLs, asCopy: true), animated: true)
            })
        } catch {
            status.text = "Cannot read the migration receipt. Originals are retained. \(error.localizedDescription)"
        }
    }
}
