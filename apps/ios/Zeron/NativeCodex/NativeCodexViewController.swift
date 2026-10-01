import UIKit
import UniformTypeIdentifiers

/// Native-only account and workspace actions around the shared session screen.
final class NativeCodexViewController: SessionViewController, UIDocumentPickerDelegate {
    private let session = NativeCodexSession.shared
    private let nativeSource: NativeCodexSessionSource
    private var shownLogin: String?
    private let workspaceExporter = NativeWorkspaceExporter()
    private weak var hostApp: AppModel?

    init(prompt: String? = nil, conversationId: String? = nil, app: AppModel? = nil) {
        let native = NativeCodexSession.shared
        if let conversationId, conversationId != native.conversation.id, let selected = native.conversations.first(where: { $0.id == conversationId }) { native.selectConversation(selected) }
        let source = NativeCodexSessionSource(conversationId: conversationId)
        nativeSource = source
        hostApp = app
        if let prompt { Drafts.save("native-codex-" + source.conversationId, prompt) }
        super.init(source: source, chatId: "native-codex-" + source.conversationId)
        source.onRefresh = { [weak self] in self?.refreshAccountUI() }
        source.onAccount = { [weak self] in self?.showAccount() }
    }
    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        if navigationController?.viewControllers.first === self {
            navigationItem.leftBarButtonItem = UIBarButtonItem(systemItem: .close, primaryAction: UIAction { [weak self] _ in self?.dismiss(animated: true) })
        }
    }

    override func sessionMenu() -> UIMenu {
        UIMenu(children: [UIDeferredMenuElement.uncached { [weak self] done in
            guard let self else { return done([]) }
            let disabled: UIMenuElement.Attributes = self.session.running || self.session.workspaceBusy || self.session.conversation.id != self.nativeSource.conversationId ? .disabled : []
            var actions: [UIMenuElement] = [
                UIAction(title: "Copy Transcript", image: UIImage(systemName: "doc.on.doc")) { [weak self] _ in
                    UIPasteboard.general.string = self?.session.conversation.messages.compactMap(\.copiedText).joined(separator: "\n\n")
                },
                UIAction(title: "Preview website", image: UIImage(systemName: "globe"), attributes: disabled) { [weak self] _ in guard let self else { return }; self.navigationController?.pushViewController(NativeWebsiteViewController(shell: self.session.shell), animated: true) },
                UIAction(title: "Workspace files", image: UIImage(systemName: "folder"), attributes: disabled) { [weak self] _ in self?.showFiles() },
                UIAction(title: self.session.signedIn ? "Native Codex account" : "Sign in with ChatGPT", image: UIImage(systemName: "person.crop.circle"), attributes: disabled) { [weak self] _ in self?.showAccount() }
            ]
            if !self.session.signedIn && self.session.login == nil && !self.session.preparingLogin {
                actions.append(UIAction(title: "Use an API key instead", attributes: disabled) { [weak self] _ in self?.enterKey() })
            }
            if let app = self.hostApp {
                let id = self.chatId
                let pinned = app.session(id)?.pinned == true
                actions.insert(UIAction(title: pinned ? "Unpin" : "Pin", image: UIImage(systemName: pinned ? "pin.slash" : "pin")) { _ in app.setPinned(id, !pinned) }, at: 0)
                actions.append(UIAction(title: "Archive", image: UIImage(systemName: "archivebox"), attributes: self.session.running ? .disabled : .destructive) { [weak self] _ in
                    app.archive(id)
                    self?.navigationController?.popViewController(animated: true)
                })
            }
            done(actions)
        }])
    }

    override func attachmentMenu() -> UIMenu {
        let disabled: UIMenuElement.Attributes = session.running || session.workspaceBusy || session.conversation.id != nativeSource.conversationId ? .disabled : []
        return UIMenu(children: [
            UIAction(title: "Import project folder", image: UIImage(systemName: "folder"), attributes: disabled) { [weak self] _ in self?.importFiles(folder: true) },
            UIAction(title: "Import files", image: UIImage(systemName: "doc"), attributes: disabled) { [weak self] _ in self?.importFiles(folder: false) },
            UIAction(title: "Save workspace to Files", image: UIImage(systemName: "square.and.arrow.up"), attributes: disabled) { [weak self] _ in self?.exportWorkspace() }
        ])
    }

    private func refreshAccountUI() {
        guard isViewLoaded, view.window != nil else { return }
        if let login = session.login, shownLogin != login.id, presentedViewController == nil {
            shownLogin = login.id
            let alert = UIAlertController(title: "Sign in to ChatGPT", message: "Enter this code: \(login.code)\nReturn here when finished.", preferredStyle: .alert)
            alert.addAction(UIAlertAction(title: "Copy code and open ChatGPT", style: .default) { _ in
                UIPasteboard.general.string = login.code
                UIApplication.shared.open(login.url)
            })
            alert.addAction(UIAlertAction(title: "Cancel sign-in", style: .cancel) { [weak self] _ in self?.session.cancelLogin() })
            present(alert, animated: true)
        }
    }

    private func showAccount() {
        guard session.ready else { session.start(); return }
        guard session.signedIn else {
            if session.login != nil { shownLogin = nil; refreshAccountUI() }
            else { session.signIn() }
            return
        }
        let alert = UIAlertController(title: "Native Codex account", message: "The agent and workspace run here. Model requests use OpenAI over the internet.", preferredStyle: .actionSheet)
        if session.signedIn {
            alert.addAction(UIAlertAction(title: "Sign out", style: .destructive) { [weak self] _ in self?.session.signOut() })
        }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.popoverPresentationController?.barButtonItem = navigationItem.rightBarButtonItem
        present(alert, animated: true)
    }

    private func enterKey() {
        let alert = UIAlertController(title: "OpenAI API key", message: "Stored in protected app storage on this device.", preferredStyle: .alert)
        alert.addTextField { $0.isSecureTextEntry = true; $0.autocapitalizationType = .none; $0.autocorrectionType = .no }
        alert.addAction(UIAlertAction(title: "Sign in", style: .default) { [weak self, weak alert] _ in
            let key = alert?.textFields?.first?.text ?? ""
            alert?.textFields?.first?.text = ""
            Task { do { try await self?.session.useAPIKey(key) } catch { self?.showError(error) } }
        })
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        present(alert, animated: true)
    }

    private func importFiles(folder: Bool) {
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: folder ? [.folder] : [.item], asCopy: false)
        picker.allowsMultipleSelection = !folder
        picker.delegate = self
        present(picker, animated: true)
    }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        guard session.conversation.id == nativeSource.conversationId, session.beginWorkspaceChange() else { return }
        let shell = session.shell
        let scoped = urls.filter { $0.startAccessingSecurityScopedResource() }
        Task {
            defer { scoped.forEach { $0.stopAccessingSecurityScopedResource() }; session.endWorkspaceChange() }
            do {
                let count = try await shell.importURLs(urls)
                if urls.count == 1, (try? urls[0].resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true { session.nameWorkspace(urls[0].lastPathComponent) }
                else { session.workspaceChanged() }
                let alert = UIAlertController(title: "Workspace imported", message: "Copied \(count) files into this chat. The original folder is unchanged. Git metadata and dependency/build folders are excluded.", preferredStyle: .alert)
                alert.addAction(UIAlertAction(title: "OK", style: .default)); present(alert, animated: true)
            } catch { showError(error) }
        }
    }

    private func exportWorkspace() {
        guard !session.running, !session.workspaceBusy, session.conversation.id == nativeSource.conversationId else { return }
        workspaceExporter.save(shell: session.shell, path: nil, from: self)
    }

    private func showFiles() {
        guard !session.running, !session.workspaceBusy, session.conversation.id == nativeSource.conversationId else { return }
        navigationController?.pushViewController(NativeWorkspaceViewController(shell: session.shell), animated: true)
    }

    private func showError(_ error: Error) {
        let alert = UIAlertController(title: "Codex", message: error.localizedDescription, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default))
        present(alert, animated: true)
    }
}

final class NativeCodexFileViewController: UIViewController {
    let editor = UITextView()
    let path: String
    let shell: MobileShellRuntime
    init(path: String, text: String, shell: MobileShellRuntime) {
        self.path = path; self.shell = shell
        super.init(nibName: nil, bundle: nil)
        editor.text = text
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func viewDidLoad() {
        super.viewDidLoad()
        title = (path as NSString).lastPathComponent
        editor.font = UIFontMetrics(forTextStyle: .body).scaledFont(for: .monospacedSystemFont(ofSize: 14, weight: .regular))
        editor.adjustsFontForContentSizeCategory = true
        editor.textContainerInset = UIEdgeInsets(top: 20, left: 16, bottom: 24, right: 16)
        editor.backgroundColor = .systemBackground
        editor.smartQuotesType = .no
        editor.smartDashesType = .no
        editor.smartInsertDeleteType = .no
        editor.autocorrectionType = .no
        editor.autocapitalizationType = .none
        view = editor
        navigationItem.rightBarButtonItem = UIBarButtonItem(title: "Save", primaryAction: UIAction { [weak self] _ in
            guard let self else { return }
            Task {
                do { try await self.shell.writeFile(self.path, content: self.editor.text); self.navigationController?.popViewController(animated: true) }
                catch {
                    let alert = UIAlertController(title: "Could not save", message: error.localizedDescription, preferredStyle: .alert)
                    alert.addAction(UIAlertAction(title: "OK", style: .default)); self.present(alert, animated: true)
                }
            }
        })
    }
}
