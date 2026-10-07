import Foundation

/// List presentation is local to an account/org and independent of scope.
struct SessionViewPreferences: Codable, Equatable {
    enum Organization: String, Codable, CaseIterable {
        case byProject, byDevice, inOneList

        var title: String {
            switch self {
            case .byProject: "By project"
            case .byDevice: "By device"
            case .inOneList: "In one list"
            }
        }

        var core: SessionOrganization {
            switch self {
            case .byProject: .byProject
            case .byDevice: .byDevice
            case .inOneList: .inOneList
            }
        }
    }

    enum Sort: String, Codable, CaseIterable {
        case lastUpdated, created

        var title: String {
            switch self {
            case .lastUpdated: "Last updated"
            case .created: "Created"
            }
        }

        var core: SessionSort {
            switch self {
            case .lastUpdated: .lastUpdated
            case .created: .created
            }
        }
    }

    var organization: Organization = .inOneList
    var sort: Sort = .lastUpdated
    var showProjectLabel = true
    var showProjectIcon = true
    var showBranch = true
    var showPullRequest = true
    var showHarness = true
    var collapsedSections = Set<String>()

    var core: SessionViewOptions {
        SessionViewOptions(organization: organization.core, sort: sort.core)
    }

    init() {}

    private enum CodingKeys: String, CodingKey {
        case organization, sort, showProjectLabel, showProjectIcon, showBranch, showPullRequest, showHarness, collapsedSections
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        organization = (try? values.decode(Organization.self, forKey: .organization)) ?? .inOneList
        sort = (try? values.decode(Sort.self, forKey: .sort)) ?? .lastUpdated
        showProjectLabel = (try? values.decode(Bool.self, forKey: .showProjectLabel)) ?? true
        showProjectIcon = (try? values.decode(Bool.self, forKey: .showProjectIcon)) ?? true
        showBranch = (try? values.decode(Bool.self, forKey: .showBranch)) ?? true
        showPullRequest = (try? values.decode(Bool.self, forKey: .showPullRequest)) ?? true
        showHarness = (try? values.decode(Bool.self, forKey: .showHarness)) ?? true
        collapsedSections = (try? values.decode(Set<String>.self, forKey: .collapsedSections)) ?? []
    }
}
