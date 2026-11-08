import UIKit

/// Activity: the Sessions page as one list of the chats that need you, the
/// one that came back last at the top (the desktop sidebar's Activity view,
/// `apps/zgx/ui/screens/shell/activity.luau`). A chat under way folds into
/// Working (running, or waiting on background work that will wake it), one
/// put to sleep into Asleep until its time or until it asks for you, and
/// archived ones sit in Archived. The three shelves stand at the list's
/// foot, folded until opened.
///
/// What a chat is doing and its sleep are the engine's (synced, the same on
/// every device: a row's status, `waiting`, `sleepUntil`, `wokeAt`). The
/// view and its settings are this phone's (`AppModel.activityOn` …). Pure:
/// the Sessions page and Settings read it.
enum ActivityList {
    enum Keys {
        static let on = "activity.on"
        static let workingShelf = "activity.workingShelf"
        static let archiveAfter = "activity.archiveAfter"
        /// Chats brought back from Archived, by id: when (Unix seconds).
        static let unarchived = "activity.unarchived"
        /// The shelves opened (they start folded).
        static let openShelves = "activity.openShelves"
    }

    /// The Sessions page's sections while Activity is on.
    enum Section {
        static let list = "activity"
        static let working = "activity.working"
        static let asleep = "activity.asleep"
        static let archived = "activity.archived"
        /// The shelves: folded until opened.
        static let shelves: Set<String> = [working, asleep, archived]
    }

    enum Shelf: Equatable {
        case working, asleep
    }

    /// Settings → Activity's choices for Archive Idle Chats, in days (0: never).
    static let archiveAfterChoices = [0, 1, 3, 7, 14, 30]

    static func archiveAfterLabel(_ days: Int) -> String {
        switch days {
        case 0: "Never"
        case 1: "After 1 Day"
        default: "After \(days) Days"
        }
    }

    /// The shelf a chat sits on, or nil for the list itself: asleep while
    /// it sleeps, Working while under way (Waiting reads `.working`) when
    /// that shelf is on.
    static func shelf(_ vm: SessionRowVM, workingShelf: Bool) -> Shelf? {
        if vm.sleepUntil != nil { return .asleep }
        if workingShelf, vm.status == .working { return .working }
        return nil
    }

    /// A row as Activity draws it: woken from sleep and not looked at since,
    /// the alarm, unless it's asking for you or under way; asleep, its corner
    /// says when it wakes.
    static func decorate(_ vm: SessionRowVM, now: Date) -> SessionRowVM {
        var vm = vm
        if vm.wokeAt != nil, vm.status == .idle || vm.status == .completed {
            vm.status = .woke
        }
        if let until = vm.sleepUntil {
            vm.timeLabel = wakesIn(until, now: now)
        }
        return vm
    }

    /// Higher stands nearer the top: the list by when it last came back (its
    /// own last move, or waking); Asleep by how soon it wakes.
    static func rank(_ vm: SessionRowVM, shelf: Shelf?) -> TimeInterval {
        if shelf == .asleep, let until = vm.sleepUntil { return -until.timeIntervalSince1970 }
        return max(vm.lastActivity, vm.wokeAt ?? .distantPast).timeIntervalSince1970
    }

    struct Page: Equatable {
        var list: [SessionRowVM] = []
        var working: [SessionRowVM] = []
        var asleep: [SessionRowVM] = []
    }

    /// The list and its shelves from the active (unpinned, unarchived)
    /// chats, each ranked, drawn as Activity draws them.
    static func page(_ rows: [SessionRowVM], workingShelf: Bool, now: Date) -> Page {
        var page = Page()
        var keyed: [(rank: TimeInterval, vm: SessionRowVM, shelf: Shelf?)] = []
        var seen = Set<String>()
        for vm in rows where seen.insert(vm.id).inserted {
            let shelf = shelf(vm, workingShelf: workingShelf)
            keyed.append((rank(vm, shelf: shelf), decorate(vm, now: now), shelf))
        }
        keyed.sort { $0.rank != $1.rank ? $0.rank > $1.rank : $0.vm.id < $1.vm.id }
        for k in keyed {
            switch k.shelf {
            case .working: page.working.append(k.vm)
            case .asleep: page.asleep.append(k.vm)
            case nil: page.list.append(k.vm)
            }
        }
        return page
    }

    /// How soon `until` is, as a row's corner says it: "45m", "3h", "2d".
    static func wakesIn(_ until: Date, now: Date) -> String {
        let left = max(0, until.timeIntervalSince(now))
        if left < 3600 { return "\(max(1, Int((left / 60).rounded(.up))))m" }
        if left < 86_400 { return "\(Int((left / 3600).rounded(.up)))h" }
        return "\(Int((left / 86_400).rounded(.up)))d"
    }

    /// One of Sleep's choices: what it's called, when it wakes as its
    /// subtitle says it, and that moment.
    struct Preset: Equatable {
        let id: String
        let title: String
        let detail: String
        let until: Date
    }

    /// Sleep's choices, soonest first, one a minute: in an hour, in three,
    /// this evening (6 PM, while it's still ahead), tomorrow at 9, next
    /// Monday at 9. Each says when it wakes: its time, and its day where its
    /// name doesn't.
    static func presets(now: Date, calendar: Calendar = .current, locale: Locale = .current) -> [Preset] {
        let today = calendar.startOfDay(for: now)
        func at(days: Int, hour: Int) -> Date? {
            calendar.date(byAdding: .day, value: days, to: today).flatMap {
                calendar.date(bySettingHour: hour, minute: 0, second: 0, of: $0)
            }
        }
        let time = DateFormatter()
        time.locale = locale
        time.calendar = calendar
        time.timeZone = calendar.timeZone
        time.dateStyle = .none
        time.timeStyle = .short
        let weekday = DateFormatter()
        weekday.locale = locale
        weekday.calendar = calendar
        weekday.timeZone = calendar.timeZone
        weekday.setLocalizedDateFormatFromTemplate("EEE")
        var out: [Preset] = []
        func add(_ id: String, _ title: String, _ until: Date?, namesDay: Bool) {
            guard let until, until > now else { return }
            let sameDay = calendar.isDate(until, inSameDayAs: now)
            let detail = namesDay || sameDay ? time.string(from: until) : "\(weekday.string(from: until)) \(time.string(from: until))"
            out.append(Preset(id: id, title: title, detail: detail, until: until))
        }
        add("hour", "In 1 Hour", now.addingTimeInterval(3600), namesDay: false)
        add("hours", "In 3 Hours", now.addingTimeInterval(3 * 3600), namesDay: false)
        add("evening", "This Evening", at(days: 0, hour: 18), namesDay: true)
        add("tomorrow", "Tomorrow", at(days: 1, hour: 9), namesDay: true)
        // Monday: a week on from a Monday; the next one from any other day.
        let toMonday = (9 - calendar.component(.weekday, from: now)) % 7
        add("week", "Next Week", at(days: toMonday == 0 ? 7 : toMonday, hour: 9), namesDay: false)
        out.sort { $0.until < $1.until }
        var minutes = Set<Int>()
        return out.filter { minutes.insert(Int($0.until.timeIntervalSince1970 / 60)).inserted }
    }

    /// Where Custom… opens: tomorrow at 9 (a time already chosen, moved
    /// from there).
    static func customStart(now: Date, calendar: Calendar = .current) -> Date {
        let tomorrow = calendar.date(byAdding: .day, value: 1, to: calendar.startOfDay(for: now)) ?? now
        return calendar.date(bySettingHour: 9, minute: 0, second: 0, of: tomorrow) ?? now.addingTimeInterval(86_400)
    }

    /// Whether auto-archive may take a chat idle long enough: never one
    /// that's pinned, open now, asleep, woken, under way or needing you.
    static func archivesWhenIdle(_ vm: SessionRowVM, open: Bool) -> Bool {
        if vm.pinned || open || vm.sleepUntil != nil || vm.wokeAt != nil || vm.sendFailed { return false }
        switch vm.status {
        case .working, .awaiting, .errored, .woke: return false
        case .idle, .completed: return true
        }
    }
}
