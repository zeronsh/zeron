"""scripts/ci/admit.py: when a waiting CI run may start."""

import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "scripts" / "ci"))

import admit  # noqa: E402
from admit import Snapshot, Waiter, decide, held, need_of, parse_reservation  # noqa: E402

BUDGET = {"total": 10, "macos": 3, "windows": 4}


def need(linux=0, windows=0, macos=0):
    return need_of({"linux": linux, "windows": windows, "macos": macos})


def waiter(run, order, running=True, **counts):
    return Waiter(run, (f"2026-10-27T00:00:{order:02d}Z", order), need(**counts), running)


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
        everything = plan.reservation({s: True for s in plan.SUITE_JOBS}, "push", "refs/heads/main")
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
        snap = Snapshot([waiter(2, 2, macos=2)], [need(linux=2, macos=2)])
        d = decide(2, need(macos=2), snap, BUDGET)
        self.assertFalse(d.admit)  # 2 + 2 macOS > 3
        snap.holding = [need(linux=2, macos=1)]
        self.assertTrue(decide(2, need(macos=2), snap, BUDGET).admit)

    def test_first_come_first_served(self):
        snap = Snapshot([waiter(5, 1, windows=3), waiter(6, 2, windows=3)], [need(windows=1)])
        self.assertTrue(decide(5, need(windows=3), snap, BUDGET).admit)
        later = decide(6, need(windows=3), snap, BUDGET)
        self.assertFalse(later.admit)
        self.assertEqual(later.position, 1)

    def test_backfill_only_with_what_nobody_ahead_waits_for(self):
        # Run 7 waits for macOS; run 8 needs only Linux and may go ahead...
        holding = [need(linux=1, macos=3)]
        snap = Snapshot([waiter(7, 1, linux=1, macos=2), waiter(8, 2, linux=2)], holding)
        self.assertFalse(decide(7, need(linux=1, macos=2), snap, BUDGET).admit)
        self.assertTrue(decide(8, need(linux=2), snap, BUDGET).admit)
        # ...but not into the total that run 7 will need.
        snap.holding = [need(linux=4, macos=3)]  # 7 held + run 7's admit job + run 7's 3 ahead + 2 > 10
        self.assertFalse(decide(8, need(linux=2), snap, BUDGET).admit)

    def test_waiting_admit_jobs_hold_a_runner(self):
        others = [waiter(i, 10 + i, running=True, linux=1) for i in range(20, 25)]
        snap = Snapshot([waiter(1, 1, linux=5)] + others, [])
        # five other admit jobs are running: 5 + 5 <= 10
        self.assertTrue(decide(1, need(linux=5), snap, BUDGET).admit)
        snap.holding = [need(linux=1)]
        self.assertFalse(decide(1, need(linux=5), snap, BUDGET).admit)

    def test_oversized_run_goes_when_the_resource_is_idle(self):
        snap = Snapshot([waiter(1, 1, macos=5)], [])
        self.assertTrue(decide(1, need(macos=5), snap, BUDGET).admit)
        snap.holding = [need(macos=1)]
        self.assertFalse(decide(1, need(macos=5), snap, BUDGET).admit)

    def test_unseen_self_queues_behind_everyone(self):
        snap = Snapshot([waiter(2, 1, linux=8)], [])
        d = decide(99, need(linux=3), snap, BUDGET)
        self.assertEqual(d.position, 1)
        self.assertFalse(d.admit)


class SchedulerTests(unittest.TestCase):
    """Snapshot building from API data, without the network."""

    class FakeApi:
        def __init__(self, runs, jobs):
            self.runs, self.jobs, self.calls = runs, jobs, []

        def get(self, path):
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
        self.assertEqual(snap.holding, [need(linux=2, macos=1)])
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
