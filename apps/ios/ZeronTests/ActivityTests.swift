import XCTest
@testable import Zeron

/// Activity's rules (the desktop's `screens/shell/activity.luau`, on iOS):
/// shelves, order, the woke alarm, Sleep's times, and what auto-archive
/// leaves alone.
final class ActivityTests: XCTestCase {
    private var calendar: Calendar = {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = TimeZone(identifier: "America/Los_Angeles")!
        return c
    }()
    private let locale = Locale(identifier: "en_US")

    private func date(_ y: Int, _ m: Int, _ d: Int, _ h: Int, _ min: Int = 0) -> Date {
        calendar.date(from: DateComponents(year: y, month: m, day: d, hour: h, minute: min))!
    }

    private func row(_ id: String, _ status: SessionRowVM.Status = .idle, last: TimeInterval = 0, sleepUntil: Date? = nil, wokeAt: Date? = nil, waiting: Bool = false, pinned: Bool = false) -> SessionRowVM {
        SessionRowVM(id: id, title: id, projectName: "p", hasProject: true, colorIndex: 0, harness: nil, branch: nil, pr: nil, prNumber: nil, status: status, timeLabel: "1h", unseen: false, pinned: pinned, sendFailed: false, waiting: waiting, sleepUntil: sleepUntil, wokeAt: wokeAt, lastActivity: Date(timeIntervalSince1970: last))
    }

    /// Formatters put a narrow no-break space before AM/PM.
    private func plain(_ s: String) -> String {
        s.replacingOccurrences(of: "\u{202F}", with: " ").replacingOccurrences(of: "\u{00A0}", with: " ")
    }

    // MARK: Shelves and order

    func testListShelvesAndOrder() {
        let now = Date(timeIntervalSince1970: 10_000)
        let rows = [
            row("old", last: 1_000),
            row("new", last: 5_000),
            row("woke", last: 500, wokeAt: Date(timeIntervalSince1970: 9_000)),
            row("running", .working, last: 9_500),
            row("waiting", .working, last: 9_400, waiting: true),
            row("later", sleepUntil: Date(timeIntervalSince1970: 50_000)),
            row("soon", .completed, sleepUntil: Date(timeIntervalSince1970: 12_000)),
        ]
        let page = ActivityList.page(rows, workingShelf: true, now: now)
        // Woke came back last: it stands at the top, with the alarm.
        XCTAssertEqual(page.list.map(\.id), ["woke", "new", "old"])
        XCTAssertEqual(page.list.first?.status, .woke)
        // Under way, waiting too, fold into Working.
        XCTAssertEqual(page.working.map(\.id), ["running", "waiting"])
        // Asleep by how soon it wakes; its corner says when.
        XCTAssertEqual(page.asleep.map(\.id), ["soon", "later"])
        XCTAssertEqual(page.asleep.map(\.timeLabel), ["34m", "12h"])
    }

    func testWorkingShelfOffKeepsWorkInTheList() {
        let rows = [row("a", .working, last: 2), row("b", last: 1)]
        let page = ActivityList.page(rows, workingShelf: false, now: Date())
        XCTAssertEqual(page.list.map(\.id), ["a", "b"])
        XCTAssertTrue(page.working.isEmpty)
    }

    func testWokeYieldsToAskingAndWork() {
        let at = Date()
        XCTAssertEqual(ActivityList.decorate(row("a", .awaiting, wokeAt: at), now: at).status, .awaiting)
        XCTAssertEqual(ActivityList.decorate(row("b", .working, wokeAt: at), now: at).status, .working)
        XCTAssertEqual(ActivityList.decorate(row("c", .completed, wokeAt: at), now: at).status, .woke)
        XCTAssertEqual(ActivityList.decorate(row("d"), now: at).status, .idle)
    }

    func testWokeCornerIsTheAlarm() {
        let corner = SessionCell.corner(row("a", .woke))
        XCTAssertEqual(corner?.word, "Woke")
        XCTAssertEqual(corner?.glyph, .symbol("alarm", StatusTone.input))
    }

    func testWaitingIsDrawnAsWorking() {
        XCTAssertEqual(AppModel.status(.waiting), .working)
        XCTAssertEqual(SessionCell.corner(row("a", .working, waiting: true))?.word, "Working")
    }

    func testDuplicatesListOnce() {
        let page = ActivityList.page([row("a"), row("a")], workingShelf: true, now: Date())
        XCTAssertEqual(page.list.map(\.id), ["a"])
    }

    // MARK: Sleep's times

    func testPresetsOnAWednesdayMorning() {
        let now = date(2026, 11, 4, 10) // a Wednesday
        let p = ActivityList.presets(now: now, calendar: calendar, locale: locale)
        XCTAssertEqual(p.map(\.id), ["hour", "hours", "evening", "tomorrow", "week"])
        XCTAssertEqual(p.map(\.until), [date(2026, 11, 4, 11), date(2026, 11, 4, 13), date(2026, 11, 4, 18), date(2026, 11, 5, 9), date(2026, 11, 9, 9)])
        XCTAssertEqual(p.map { plain($0.detail) }, ["11:00 AM", "1:00 PM", "6:00 PM", "9:00 AM", "Mon 9:00 AM"])
    }

    func testPresetsKeepOneAMinute() {
        // In three hours is this evening: one row.
        let p = ActivityList.presets(now: date(2026, 11, 4, 15), calendar: calendar, locale: locale)
        XCTAssertEqual(p.filter { $0.until == date(2026, 11, 4, 18) }.count, 1)
    }

    func testEveningOnlyWhileAhead() {
        let p = ActivityList.presets(now: date(2026, 11, 4, 19), calendar: calendar, locale: locale)
        XCTAssertFalse(p.contains { $0.id == "evening" })
        // Late enough that three hours on is tomorrow: it names the day.
        let late = ActivityList.presets(now: date(2026, 11, 4, 23), calendar: calendar, locale: locale)
        XCTAssertEqual(late.first { $0.id == "hours" }.map { plain($0.detail) }, "Thu 2:00 AM")
    }

    func testNextWeekFromAMondayIsAWeekOn() {
        let p = ActivityList.presets(now: date(2026, 11, 9, 8), calendar: calendar, locale: locale)
        XCTAssertEqual(p.first { $0.id == "week" }?.until, date(2026, 11, 16, 9))
    }

    func testPresetsAcrossDaylightSaving() {
        // Nov 1 2026: clocks go back in Los Angeles. Tomorrow at 9 is 9 on the wall.
        let p = ActivityList.presets(now: date(2026, 10, 31, 22), calendar: calendar, locale: locale)
        XCTAssertEqual(calendar.component(.hour, from: p.first { $0.id == "tomorrow" }!.until), 9)
    }

    func testCustomStartsTomorrowAtNine() {
        XCTAssertEqual(ActivityList.customStart(now: date(2026, 11, 4, 22), calendar: calendar), date(2026, 11, 5, 9))
    }

    func testWakesIn() {
        let now = Date(timeIntervalSince1970: 0)
        XCTAssertEqual(ActivityList.wakesIn(Date(timeIntervalSince1970: 10), now: now), "1m")
        XCTAssertEqual(ActivityList.wakesIn(Date(timeIntervalSince1970: 45 * 60), now: now), "45m")
        XCTAssertEqual(ActivityList.wakesIn(Date(timeIntervalSince1970: 3 * 3600 - 60), now: now), "3h")
        XCTAssertEqual(ActivityList.wakesIn(Date(timeIntervalSince1970: 2 * 86_400), now: now), "2d")
    }

    // MARK: Auto-archive

    func testAutoArchiveLeavesWhatNeedsYouAlone() {
        XCTAssertTrue(ActivityList.archivesWhenIdle(row("idle"), open: false))
        XCTAssertTrue(ActivityList.archivesWhenIdle(row("done", .completed), open: false))
        XCTAssertFalse(ActivityList.archivesWhenIdle(row("open"), open: true))
        XCTAssertFalse(ActivityList.archivesWhenIdle(row("pinned", pinned: true), open: false))
        XCTAssertFalse(ActivityList.archivesWhenIdle(row("asleep", sleepUntil: Date()), open: false))
        XCTAssertFalse(ActivityList.archivesWhenIdle(row("woke", wokeAt: Date()), open: false))
        for status: SessionRowVM.Status in [.working, .awaiting, .errored] {
            XCTAssertFalse(ActivityList.archivesWhenIdle(row("busy", status), open: false))
        }
    }

    func testArchiveChoices() {
        XCTAssertEqual(ActivityList.archiveAfterChoices.map(ActivityList.archiveAfterLabel), ["Never", "After 1 Day", "After 3 Days", "After 7 Days", "After 14 Days", "After 30 Days"])
    }
}
