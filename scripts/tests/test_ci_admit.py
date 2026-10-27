"""scripts/ci/admit.py: when a waiting CI run may start."""

import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "scripts" / "ci"))

import admit  # noqa: E402
from admit import Holding, Snapshot, Waiter, decide, held, need_of, parse_reservation  # noqa: E402

BUDGET = {"total": 10, "macos": 3, "windows": 4}


def need(linux=0, windows=0, macos=0):
    return need_of({"linux": linux, "windows": windows, "macos": macos})


def held_(pr=None, **counts):
    return Holding(need(**counts), pr)


def waiter(run, order, running=True, pr=None, **counts):
    return Waiter(run, (f"2026-10-27T00:00:{order:02d}Z", order), need(**counts), running, pr)


def job(name, label, status="completed", conclusion="success"):
    return {"name": name, "labels": [label], "status": status, "conclusion": conclusion}


class ReservationTests(unittest.TestCase):
    def test_parse(self):
        self.assertEqual(parse_reservation("admit [linux=3 windows=2 macos=1]"),
                         {"linux": 3, "windows": 2, "macos": 1})
        self.assertEqual(parse_reservation("admit [linux=1]"), {"linux": 1, "windows": 0, "macos": 0})
        self.assertIsNone(parse_reservation("admit []"))

    def test_reservations_are_capped_at_a_full_run(self):
        # Another run's admit name comes from its own (editable) ci.yml.
        self.assertEqual(parse_reservation("admit [linux=999 windows=50 macos=40]"), admit.MAX_RESERVATION)

    def test_cap_is_the_largest_plan(self):
        sys.path.insert(0, str(pathlib.Path(admit.__file__).parent))
        import plan
        everything = plan.reservation(plan.jobs_to_run({s: True for s in plan.SUITE_JOBS}), "push",
                                      "refs/heads/main")
        self.assertEqual(everything, admit.MAX_RESERVATION)

    def test_held_releases_finished_jobs(self):
        declared = {"linux": 3, "windows": 2, "macos": 1}
        jobs = [
            job("plan", "ubuntu-24.04"),
            job("admit [linux=3 windows=2 macos=1]", "ubuntu-24.04"),
            job("linux / core-tests", "ubuntu-24.04"),
            job("windows / tests", "windows-2022", "in_progress", None),
            job("macos / macos-native", "macos-latest", conclusion="failure"),
            job("macos / macos-frame-source", "macos-latest", conclusion="skipped"),
        ]
        self.assertEqual(held(declared, jobs), {"linux": 2, "windows": 2, "macos": 0})

    def test_jobs_that_never_appear_stop_counting(self):
        declared = {"linux": 2, "windows": 0, "macos": 3}
        jobs = [job("macos / macos-native", "macos-latest", "in_progress", None)]
        self.assertEqual(held(declared, jobs, since_admitted=60)["macos"], 3)
        self.assertEqual(held(declared, jobs, since_admitted=admit.UNSEEN_GRACE + 1),
                         {"linux": 0, "windows": 0, "macos": 1})

    def test_seconds_since(self):
        import time
        stamp = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() - 120))
        self.assertAlmostEqual(admit.seconds_since(stamp), 120, delta=2)
        self.assertEqual(admit.seconds_since(None), 0)

    def test_held_never_below_what_is_running(self):
        jobs = [job(f"w{i}", "windows-2022", "in_progress", None) for i in range(3)]
        self.assertEqual(held({"linux": 0, "windows": 2, "macos": 0}, jobs)["windows"], 3)


class DecideTests(unittest.TestCase):
    def test_alone_starts(self):
        snap = Snapshot([waiter(1, 1, linux=3, windows=2, macos=2)], [])
        self.assertTrue(decide(1, need(linux=3, windows=2, macos=2), snap, BUDGET).admit)

    def test_waits_until_everything_fits(self):
        snap = Snapshot([waiter(2, 2, macos=2)], [held_(linux=2, macos=2)])
        d = decide(2, need(macos=2), snap, BUDGET)
        self.assertFalse(d.admit)  # 2 + 2 macOS > 3
        snap.holding = [held_(linux=2, macos=1)]
        self.assertTrue(decide(2, need(macos=2), snap, BUDGET).admit)

    def test_first_come_first_served(self):
        snap = Snapshot([waiter(5, 1, windows=3), waiter(6, 2, windows=3)], [held_(windows=1)])
        self.assertTrue(decide(5, need(windows=3), snap, BUDGET).admit)
        later = decide(6, need(windows=3), snap, BUDGET)
        self.assertFalse(later.admit)
        self.assertEqual(later.position, 1)

    def test_backfill_only_with_what_nobody_ahead_waits_for(self):
        # Run 7 waits for macOS; run 8 needs only Linux and may go ahead...
        holding = [held_(linux=1, macos=3)]
        snap = Snapshot([waiter(7, 1, linux=1, macos=2), waiter(8, 2, linux=2)], holding)
        self.assertFalse(decide(7, need(linux=1, macos=2), snap, BUDGET).admit)
        self.assertTrue(decide(8, need(linux=2), snap, BUDGET).admit)
        # ...but not into the total that run 7 will need.
        snap.holding = [held_(linux=4, macos=3)]  # 7 held + run 7's admit job + run 7's 3 ahead + 2 > 10
        self.assertFalse(decide(8, need(linux=2), snap, BUDGET).admit)

    def test_waiting_admit_jobs_hold_a_runner(self):
        others = [waiter(i, 10 + i, running=True, linux=1) for i in range(20, 25)]
        snap = Snapshot([waiter(1, 1, linux=5)] + others, [])
        # five other admit jobs are running: 5 + 5 <= 10
        self.assertTrue(decide(1, need(linux=5), snap, BUDGET).admit)
        snap.holding = [held_(linux=1)]
        self.assertFalse(decide(1, need(linux=5), snap, BUDGET).admit)

    def test_oversized_run_goes_when_the_resource_is_idle(self):
        snap = Snapshot([waiter(1, 1, macos=5)], [])
        self.assertTrue(decide(1, need(macos=5), snap, BUDGET).admit)
        snap.holding = [held_(macos=1)]
        self.assertFalse(decide(1, need(macos=5), snap, BUDGET).admit)

    def test_unseen_self_queues_behind_everyone(self):
        snap = Snapshot([waiter(2, 1, linux=8)], [])
        d = decide(99, need(linux=3), snap, BUDGET)
        self.assertEqual(d.position, 1)
        self.assertFalse(d.admit)


class PriorityAndLockTests(unittest.TestCase):
    BIG = {"total": 50, "macos": 10, "windows": 16}

    def test_priority_goes_first(self):
        snap = Snapshot([waiter(1, 1, linux=2, macos=3), waiter(2, 2, pr=77, linux=2, macos=3)],
                        [held_(macos=6)], priority={77})
        self.assertTrue(decide(2, need(linux=2, macos=3), snap, self.BIG).admit)
        first = decide(1, need(linux=2, macos=3), snap, self.BIG)
        self.assertFalse(first.admit)  # 6 held + 3 for #77 ahead + 3 > 10
        self.assertEqual(first.position, 1)

    def test_others_backfill_only_around_priority(self):
        snap = Snapshot([waiter(2, 1, pr=77, macos=4), waiter(3, 2, linux=3)], [held_(macos=8)], priority={77})
        self.assertFalse(decide(2, need(macos=4), snap, self.BIG).admit)  # waits for macOS
        self.assertTrue(decide(3, need(linux=3), snap, self.BIG).admit)  # Linux only: fine

    def test_a_lock_holds_a_full_run_while_idle(self):
        full = need_of(admit.MAX_RESERVATION)
        snap = Snapshot([waiter(5, 1, linux=3, macos=2)], [held_(macos=6)], locked={90})
        d = decide(5, need(linux=3, macos=2), snap, self.BIG)
        self.assertEqual(d.used["macos"], 6 + full["macos"])
        self.assertFalse(d.admit)  # 6 + 3 locked + 2 > 10
        snap.holding = [held_(macos=5)]
        self.assertTrue(decide(5, need(linux=3, macos=2), snap, self.BIG).admit)

    def test_the_locked_pull_request_goes_ahead_of_everyone_into_its_reservation(self):
        others = [waiter(i, i, linux=5, macos=3) for i in range(1, 4)]
        snap = Snapshot(others + [waiter(9, 9, pr=90, linux=10, windows=3, macos=4)],
                        [held_(linux=20, macos=6)], locked={90}, priority={1})
        d = decide(9, need(linux=10, windows=3, macos=4), snap, self.BIG)
        self.assertTrue(d.admit)
        self.assertEqual(d.position, 0)

    def test_the_lock_is_not_counted_twice_once_its_run_holds_runners(self):
        snap = Snapshot([waiter(5, 1, macos=2)], [held_(pr=90, macos=4), held_(macos=4)], locked={90})
        self.assertEqual(decide(5, need(macos=2), snap, self.BIG).used["macos"], 8)

    def test_an_unverified_claim_is_an_ordinary_run(self):
        snap = Snapshot([waiter(1, 1, linux=2), waiter(2, 2, pr=None, linux=2)], [], priority={77})
        self.assertEqual(decide(2, need(linux=2), snap, self.BIG).position, 1)


class SchedulerLabelTests(unittest.TestCase):
    class FakeApi:
        def __init__(self, pulls, labels, events):
            self.pulls, self.labels, self.events = pulls, labels, events

        def get(self, path):
            if "/issues?" in path:
                label = "ci:lock" if "ci%3Alock" in path else "ci:priority"
                return [{"number": n, "pull_request": {}} for n in self.labels.get(label, [])]
            if "/events" in path:
                number = int(path.split("/issues/")[1].split("/")[0])
                return self.events.get(number, [])
            if "/pulls/" in path:
                return self.pulls[int(path.rsplit("/", 1)[1])]
            raise AssertionError(path)

    def test_claims_are_verified_and_locks_expire(self):
        import time
        now = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        old = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() - 7 * 3600))
        lock_event = lambda when: [{"event": "labeled", "label": {"name": "ci:lock"}, "created_at": when}]  # noqa: E731
        api = self.FakeApi({77: {"head": {"sha": "abc"}}},
                           {"ci:priority": [77], "ci:lock": [90, 91]},
                           {90: lock_event(now), 91: lock_event(old)})
        scheduler = admit.Scheduler(api, "o/r", 1)
        self.assertEqual(scheduler.locks(), {90})
        run = {"event": "pull_request", "head_sha": "abc"}
        self.assertEqual(scheduler.verified_pr("admit [linux=2] #77", run, {77}), 77)
        self.assertIsNone(scheduler.verified_pr("admit [linux=2] #77", dict(run, head_sha="zzz"), {77}))
        self.assertIsNone(scheduler.verified_pr("admit [linux=2] #12", run, {77}))
        self.assertIsNone(scheduler.verified_pr("admit [linux=2] #77", dict(run, event="push"), {77}))


class EmergencyTests(unittest.TestCase):
    def test_cancels_everything_but_the_locked_pull_request(self):
        import emergency
        runs = [
            {"id": 1, "status": "in_progress", "head_sha": "mine", "event": "pull_request"},
            {"id": 2, "status": "queued", "head_sha": "other", "event": "pull_request"},
            {"id": 3, "status": "in_progress", "head_sha": "main", "event": "push"},
            {"id": 4, "status": "completed", "head_sha": "old", "event": "pull_request"},
        ]
        self.assertEqual([r["id"] for r in emergency.to_cancel(runs, "mine", False)], [2])
        self.assertEqual([r["id"] for r in emergency.to_cancel(runs, "mine", True)], [2, 3])

    def test_reruns_only_when_nothing_is_running_and_it_did_not_pass(self):
        import emergency
        failed = {"id": 5, "status": "completed", "conclusion": "cancelled", "head_sha": "h", "event": "pull_request"}
        passed = dict(failed, id=6, conclusion="success")
        running = dict(failed, id=7, status="in_progress")
        self.assertEqual(emergency.to_rerun([failed], "h")["id"], 5)
        self.assertIsNone(emergency.to_rerun([failed, passed], "h"))  # the latest passed
        self.assertIsNone(emergency.to_rerun([failed, running], "h"))
        self.assertIsNone(emergency.to_rerun([], "h"))


class SchedulerTests(unittest.TestCase):
    """Snapshot building from API data, without the network."""

    class FakeApi:
        def __init__(self, runs, jobs):
            self.runs, self.jobs, self.calls = runs, jobs, []

        def get(self, path):
            if "/issues?" in path:
                return []  # no ci:priority or ci:lock labels
            self.calls.append(path)
            if "/workflows/" in path:
                return {"workflow_runs": self.runs}
            run = int(path.split("/runs/")[1].split("/")[0])
            return {"jobs": self.jobs[run]}

    def test_snapshot_and_caching(self):
        runs = [{"id": 1, "updated_at": "a"}, {"id": 2, "updated_at": "a"}, {"id": 3, "updated_at": "a"}]
        jobs = {
            1: [job("plan", "ubuntu-24.04"), job("admit [linux=2 macos=1]", "ubuntu-24.04"),
                job("macos / macos-native", "macos-latest", "in_progress", None)],
            2: [dict(job("admit [linux=1 windows=2]", "ubuntu-24.04", "in_progress", None),
                     created_at="2026-10-27T00:00:01Z", id=20)],
            3: [job("plan", "ubuntu-24.04", "in_progress", None)],
        }
        api = self.FakeApi(runs, jobs)
        scheduler = admit.Scheduler(api, "o/r", 9)
        snap = scheduler.snapshot()
        self.assertEqual([h.need for h in snap.holding], [need(linux=2, macos=1)])
        self.assertEqual([w.run_id for w in snap.waiters], [2])
        self.assertEqual(snap.waiters[0].need, need(linux=1, windows=2))
        self.assertEqual(len(api.calls), 4)
        scheduler.snapshot()  # nothing changed: only the run list is fetched again
        self.assertEqual(len(api.calls), 5)
        runs[0]["updated_at"] = "b"
        scheduler.snapshot()
        self.assertEqual(len(api.calls), 7)


if __name__ == "__main__":
    unittest.main()
