import UIKit

/// The picker and grouped list use the same project actions and confirmations.
extension UIViewController {
    func projectActionsMenu(app: AppModel, projectId: String) -> UIMenu? {
        guard app.projectOptions.contains(where: { $0.id == projectId }) else { return nil }
        return UIMenu(children: [
            UIAction(title: "Rename project…", image: UIImage(systemName: "pencil")) { [weak self] _ in
                self?.renameProject(app: app, projectId: projectId)
            },
            UIAction(title: "Delete project…", image: UIImage(systemName: "trash"), attributes: .destructive) { [weak self] _ in
                self?.confirmProjectDeletion(app: app, projectId: projectId)
            },
        ])
    }

    private func renameProject(app: AppModel, projectId: String) {
        guard let project = app.projectOptions.first(where: { $0.id == projectId }) else {
            showProjectActionError(AppModel.ProjectActionError.removed)
            return
        }
        let alert = UIAlertController(title: "Rename project", message: "\(project.deviceName)\n\(project.path)", preferredStyle: .alert)
        alert.view.accessibilityIdentifier = "project-rename-dialog"
        let save = UIAlertAction(title: "Rename", style: .default) { [weak self, weak alert] _ in
            guard let name = alert?.textFields?.first?.text?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty else { return }
            do { try app.renameProject(projectId, name: name) }
            catch { self?.showProjectActionError(error) }
        }
        alert.addTextField { field in
            field.text = project.name
            field.placeholder = "Project name"
            field.accessibilityIdentifier = "project-name"
            field.autocapitalizationType = .none
            field.addAction(UIAction { [weak field, weak save] _ in
                save?.isEnabled = !(field?.text?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true)
            }, for: .editingChanged)
        }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(save)
        showProjectDialog(alert)
    }

    private func confirmProjectDeletion(app: AppModel, projectId: String) {
        guard let summary = app.projectDeletionSummary(projectId) else {
            showProjectActionError(AppModel.ProjectActionError.removed)
            return
        }
        let message = "This removes “\(summary.projectName)” and all \(summary.sessionCount) sessions from the workspace, including \(summary.archivedCount) archived sessions.\n\nThe project folder and its files stay on the host."
        let alert = UIAlertController(title: "Delete project?", message: message, preferredStyle: .alert)
        alert.view.accessibilityIdentifier = "project-delete-dialog"
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(UIAlertAction(title: "Delete project", style: .destructive) { [weak self] _ in
            guard let self else { return }
            guard let current = app.projectDeletionSummary(projectId) else {
                self.showProjectActionError(AppModel.ProjectActionError.removed)
                return
            }
            // A newly synced session changes the impact the user is approving.
            guard current.sessionCount == summary.sessionCount,
                  current.archivedCount == summary.archivedCount,
                  current.projectName == summary.projectName else {
                self.confirmProjectDeletion(app: app, projectId: projectId)
                return
            }
            do { try app.deleteProject(projectId) }
            catch { self.showProjectActionError(error) }
        })
        showProjectDialog(alert)
    }

    private func showProjectActionError(_ error: Error) {
        let alert = UIAlertController(title: "Project action failed", message: error.localizedDescription, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default))
        showProjectDialog(alert)
    }

    private func showProjectDialog(_ alert: UIAlertController) {
        var presenter = self
        while let next = presenter.presentedViewController { presenter = next }
        if presenter is UIAlertController {
            presenter.dismiss(animated: true) { [weak self] in self?.showProjectDialog(alert) }
        } else {
            presenter.present(alert, animated: true)
        }
    }
}
