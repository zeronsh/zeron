import UIKit

/// Temporary until the data core lands: every tab screen takes the app model.
final class AppModel {}

class PlaceholderViewController: UIViewController {
    init(app: AppModel, title: String) {
        super.init(nibName: nil, bundle: nil)
        self.title = title
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
    }
}

final class ProjectsViewController: PlaceholderViewController { init(app: AppModel) { super.init(app: app, title: "Projects") }; required init?(coder: NSCoder) { fatalError() } }
final class SessionsViewController: PlaceholderViewController { init(app: AppModel) { super.init(app: app, title: "Sessions") }; required init?(coder: NSCoder) { fatalError() } }
final class PullRequestsViewController: PlaceholderViewController { init(app: AppModel) { super.init(app: app, title: "Pull Requests") }; required init?(coder: NSCoder) { fatalError() } }
final class MoreViewController: PlaceholderViewController { init(app: AppModel) { super.init(app: app, title: "More") }; required init?(coder: NSCoder) { fatalError() } }
final class SearchViewController: PlaceholderViewController { init(app: AppModel) { super.init(app: app, title: "Search") }; required init?(coder: NSCoder) { fatalError() } }
final class SessionViewController: PlaceholderViewController { init(app: AppModel, chatId: String) { super.init(app: app, title: "Session") }; required init?(coder: NSCoder) { fatalError() } }
final class NewSessionViewController: PlaceholderViewController { init(app: AppModel, prompt: String?, onCreated: @escaping (String) -> Void) { super.init(app: app, title: "New Session") }; required init?(coder: NSCoder) { fatalError() } }
