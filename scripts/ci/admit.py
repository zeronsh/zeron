#!/usr/bin/env python3
"""Admission control for CI runs: a run starts only when all its runners are free.

GitHub queues every job of every run as soon as it can, so under load each run
gets a few of its jobs going and holds those runners while its other jobs wait
behind other runs' jobs: every run is slow, none finishes. Instead, the
`admit` job of .github/workflows/ci.yml runs this script, which waits until
the runners the whole run needs (its reservation, computed by plan.py) fit in
the runner budget, and only then lets the run's jobs start.

Bookkeeping lives in GitHub itself, so nothing needs write access (pull
requests from forks only get a read-only token):

  - A run's reservation is in its admit job's name: "admit [linux=3 windows=2 macos=1]".
  - A run whose admit job is still queued or running is waiting. Waiting runs
    are served first come, first served, ordered by when their admit job was
    created (a re-run queues again at the back).
  - A run whose admit job succeeded holds, per runner type, its reservation
    minus the jobs of that type that have finished: a runner is released as
    soon as the job using it is done, not when the whole run ends.
  - A waiting run's own admit job holds one Linux runner.

Any pull request can edit its own copy of ci.yml, so a reservation read from
another run is not trusted: it is capped at the most plan.py ever reserves
(MAX_RESERVATION), and an admitted run's jobs that never show up stop counting
UNSEEN_GRACE after admission. A run cannot hold more than a full run's worth
of runners, or hold runners it never uses for long.

A waiting run is admitted when, for every resource it needs, what admitted
runs hold + what the runs ahead of it in the queue need + its own need fits in
the budget. So a run that needs only Linux can go ahead of a run waiting for
a macOS runner, but never takes anything that run is waiting for (conservative
backfilling: nobody is overtaken on a resource they wait for).

Resources: `total` (every hosted job, the org-wide concurrency limit), and
`macos` and `windows`. Budgets come from repository variables
CI_BUDGET_TOTAL / CI_BUDGET_MACOS / CI_BUDGET_WINDOWS. They should leave
headroom for runners this script cannot see (other repositories of the
organization, release and deploy workflows), which is also why admission is
"all runners were free when the run started", not a hard guarantee.

The GITHUB_TOKEN may make 1000 API requests an hour per repository, shared by
every job. The script uses conditional requests (a 304 is free), refetches a
run's jobs only when the run changed, polls less often the further back in the
queue it is, and slows down when the remaining quota is low. If it cannot
reach the API, the quota resets more than 10 minutes away, or it waited
MAX_WAIT_MINUTES, it lets the run start (fail open) rather than block CI.
"""

from __future__ import annotations

import calendar
import json
import os
import re
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field

RUNNERS = ("linux", "windows", "macos")
RESOURCES = ("total", "macos", "windows")
ADMIT_PREFIX = "admit"
UNRESERVED_JOBS = ("plan",)  # jobs that run before admission and are not in reservations
RESERVATION = re.compile(r"\b(linux|windows|macos)=(\d+)\b")
# The largest reservation plan.py makes: every suite, on main (checked by test_ci_admit.py).
MAX_RESERVATION = {"linux": 10, "windows": 3, "macos": 4}
# How long an admitted run's not-yet-created jobs stay reserved. Jobs that wait for
# another job (iOS simulator tests, Android publishing) are created within minutes.
UNSEEN_GRACE = 90 * 60


def parse_reservation(name: str) -> dict[str, int] | None:
    found = dict((k, int(v)) for k, v in RESERVATION.findall(name))
    if not found:
        return None
    return {r: min(found.get(r, 0), MAX_RESERVATION[r]) for r in RUNNERS}


def need_of(counts: dict[str, int]) -> dict[str, int]:
    return {"total": sum(counts.values()), "macos": counts["macos"], "windows": counts["windows"]}


def runner_of(job: dict) -> str:
    labels = " ".join(job.get("labels") or []).lower()
    if "windows" in labels:
        return "windows"
    if "macos" in labels:
        return "macos"
    return "linux"


def is_admit(job: dict) -> bool:
    return job["name"].startswith(ADMIT_PREFIX)


def held(declared: dict[str, int], jobs: list[dict], since_admitted: float = 0) -> dict[str, int]:
    """Runners an admitted run still holds: its reservation minus its finished jobs.

    After UNSEEN_GRACE only jobs that exist and have not finished count.
    """
    done = {r: 0 for r in RUNNERS}
    active = {r: 0 for r in RUNNERS}
    for job in jobs:
        if is_admit(job) or job["name"] in UNRESERVED_JOBS:
            continue
        # Skipped jobs never had a runner, and the plan did not count the ones it turned
        # off. (One skipped because a job it needs failed stays reserved until the run ends.)
        if job.get("conclusion") == "skipped":
            continue
        bucket = done if job["status"] == "completed" else active
        bucket[runner_of(job)] += 1
    if since_admitted > UNSEEN_GRACE:
        return active
    return {r: max(declared[r] - done[r], active[r], 0) for r in RUNNERS}


def seconds_since(timestamp: str | None) -> float:
    if not timestamp:
        return 0
    return time.time() - calendar.timegm(time.strptime(timestamp, "%Y-%m-%dT%H:%M:%SZ"))


@dataclass
class Waiter:
    run_id: int
    key: tuple  # (admit job created_at, admit job id): queue order
    need: dict[str, int]
    running: bool  # its admit job holds a runner


@dataclass
class Snapshot:
    waiters: list[Waiter] = field(default_factory=list)
    holding: list[dict[str, int]] = field(default_factory=list)  # need_of(held()) per admitted run


@dataclass
class Decision:
    admit: bool
    position: int
    used: dict[str, int]
    ahead: dict[str, int]


def decide(me: int, mine: dict[str, int], snapshot: Snapshot, budget: dict[str, int]) -> Decision:
    used = {r: 0 for r in RESOURCES}
    for h in snapshot.holding:
        for r in RESOURCES:
            used[r] += h[r]
    for w in snapshot.waiters:
        if w.run_id != me and w.running:
            used["total"] += 1

    ahead = {r: 0 for r in RESOURCES}
    position = 0
    order = sorted(snapshot.waiters, key=lambda w: w.key)
    if any(w.run_id == me for w in order):
        for w in order:
            if w.run_id == me:
                break
            position += 1
            for r in RESOURCES:
                ahead[r] += w.need[r]
    else:  # not visible yet (API lag): behind everyone
        position = len(order)
        for w in order:
            for r in RESOURCES:
                ahead[r] += w.need[r]

    # A run larger than the budget goes when nothing else holds or waits for that resource.
    fits = all(used[r] + ahead[r] + min(mine[r], budget[r]) <= budget[r]
               for r in RESOURCES if mine[r] > 0)
    return Decision(fits, position, used, ahead)


# GitHub API ------------------------------------------------------------------------------------

class RateLimited(Exception):
    def __init__(self, wait: float):
        super().__init__(f"rate limited for {wait:.0f}s")
        self.wait = wait


class Api:
    def __init__(self, token: str, base: str):
        self.token = token
        self.base = base
        self.cache: dict[str, tuple[str, object]] = {}
        self.remaining = 1000
        self.requests = 0

    def get(self, path: str):
        url = self.base + path
        headers = {
            "Authorization": f"Bearer {self.token}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
        }
        cached = self.cache.get(url)
        if cached:
            headers["If-None-Match"] = cached[0]
        request = urllib.request.Request(url, headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                self._note(response.headers)
                body = json.load(response)
                etag = response.headers.get("ETag")
                if etag:
                    self.cache[url] = (etag, body)
                return body
        except urllib.error.HTTPError as error:
            self._note(error.headers)
            if error.code == 304 and cached:
                return cached[1]
            if error.code in (403, 429):
                retry = error.headers.get("Retry-After")
                reset = error.headers.get("X-RateLimit-Reset")
                if retry:
                    raise RateLimited(float(retry)) from error
                if error.headers.get("X-RateLimit-Remaining") == "0" and reset:
                    raise RateLimited(max(30.0, float(reset) - time.time())) from error
            raise

    def _note(self, headers) -> None:
        if headers is None:
            return
        remaining = headers.get("X-RateLimit-Remaining")
        if remaining is not None:
            self.remaining = int(remaining)
            self.requests += 1


class Scheduler:
    """Builds Snapshots, refetching a run's jobs only when the run changed."""

    REFRESH = 300  # refetch unchanged runs' jobs at least this often (seconds)

    def __init__(self, api: Api, repo: str, workflow_id: int):
        self.api = api
        self.repo = repo
        self.workflow_id = workflow_id
        self.jobs: dict[int, tuple[str, float, list[dict]]] = {}  # run -> (updated_at, fetched, jobs)

    def run_jobs(self, run: dict) -> list[dict]:
        cached = self.jobs.get(run["id"])
        now = time.time()
        if cached and cached[0] == run["updated_at"] and now - cached[1] < self.REFRESH:
            return cached[2]
        jobs: list[dict] = []
        page = 1
        while True:
            data = self.api.get(f"/repos/{self.repo}/actions/runs/{run['id']}/jobs"
                                f"?filter=latest&per_page=100&page={page}")
            jobs.extend(data["jobs"])
            if len(data["jobs"]) < 100:
                break
            page += 1
        self.jobs[run["id"]] = (run["updated_at"], now, jobs)
        return jobs

    def snapshot(self) -> Snapshot:
        runs: list[dict] = []
        page = 1
        while True:
            data = self.api.get(f"/repos/{self.repo}/actions/workflows/{self.workflow_id}/runs"
                                f"?status=in_progress&per_page=100&page={page}")
            runs.extend(data["workflow_runs"])
            if len(data["workflow_runs"]) < 100:
                break
            page += 1
        live = {run["id"] for run in runs}
        self.jobs = {k: v for k, v in self.jobs.items() if k in live}

        snap = Snapshot()
        for run in runs:
            jobs = self.run_jobs(run)
            admit = next((j for j in jobs if is_admit(j)), None)
            if admit is None:
                continue  # still planning: not asking for anything yet
            declared = parse_reservation(admit["name"])
            if declared is None:
                continue
            if admit["status"] != "completed":
                snap.waiters.append(Waiter(run["id"], (admit["created_at"], admit["id"]),
                                           need_of(declared), admit["status"] == "in_progress"))
            elif admit["conclusion"] == "success":
                snap.holding.append(need_of(held(declared, jobs, seconds_since(admit.get("completed_at")))))
        return snap


def env_int(name: str, default: int) -> int:
    value = os.environ.get(name, "").strip()
    return int(value) if value.isdigit() else default


def summary(text: str) -> None:
    print(text)
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    if path:
        with open(path, "a") as f:
            f.write(text + "\n")


def main() -> int:
    mine_counts = parse_reservation(os.environ["CI_RESERVATION"])
    if mine_counts is None:
        print(f"::warning::unreadable reservation {os.environ['CI_RESERVATION']!r}; not waiting")
        return 0
    mine = need_of(mine_counts)
    if os.environ.get("CI_ADMISSION", "on").lower() in ("off", "false", "0"):
        summary("Admission control is off (CI_ADMISSION); starting at once.")
        return 0
    if mine["total"] <= 1:
        summary("Nothing to test but the result job; starting at once.")
        return 0

    budget = {
        "total": env_int("CI_BUDGET_TOTAL", 50),
        "macos": env_int("CI_BUDGET_MACOS", 10),
        "windows": env_int("CI_BUDGET_WINDOWS", 16),
    }
    max_wait = env_int("MAX_WAIT_MINUTES", 300) * 60
    debounce = env_int("CI_DEBOUNCE_SECONDS", 60) if os.environ.get("GITHUB_EVENT_NAME") == "pull_request" else 0

    repo = os.environ["GITHUB_REPOSITORY"]
    me = int(os.environ["GITHUB_RUN_ID"])
    api = Api(os.environ["GITHUB_TOKEN"], os.environ.get("GITHUB_API_URL", "https://api.github.com"))
    started = time.time()

    if debounce:
        # A newer push to the same pull request cancels this run (ci.yml's concurrency group):
        # pushes in quick succession only start the last one.
        print(f"Waiting {debounce}s for further pushes")
        time.sleep(debounce)

    try:
        workflow_id = api.get(f"/repos/{repo}/actions/runs/{me}")["workflow_id"]
    except (OSError, RateLimited, KeyError, ValueError) as error:
        print(f"::warning::admission unavailable ({error}); starting at once")
        return 0
    scheduler = Scheduler(api, repo, workflow_id)

    print(f"Reservation {mine}; budget {budget}")
    failures = 0
    last = None
    while True:
        waited = time.time() - started
        try:
            snap = scheduler.snapshot()
            failures = 0
        except RateLimited as error:
            print(f"::warning::{error}")
            if error.wait > 600 or waited + error.wait > max_wait:
                summary("::warning::Admission gave up (API rate limit); starting without a reservation.")
                return 0
            time.sleep(error.wait)
            continue
        except (OSError, KeyError, ValueError) as error:  # URLError, HTTPError, timeouts, bad JSON
            failures += 1
            print(f"::warning::admission poll failed ({error})")
            if failures >= 5:
                summary("::warning::Admission unavailable (API errors); starting without a reservation.")
                return 0
            time.sleep(30)
            continue

        decision = decide(me, mine, snap, budget)
        state = (decision.position, tuple(decision.used.values()), tuple(decision.ahead.values()))
        if state != last:
            print(f"[{waited / 60:5.1f} min] position {decision.position}, in use {decision.used}, "
                  f"needed ahead {decision.ahead}, {len(snap.waiters)} waiting, "
                  f"API quota left {api.remaining}")
            last = state
        if decision.admit:
            summary(f"Admitted after {waited / 60:.1f} min (reservation `{os.environ['CI_RESERVATION']}`, "
                    f"in use {decision.used}, budget {budget}, {api.requests} API requests).")
            return 0
        if waited > max_wait:
            summary(f"::warning::Admission waited {waited / 60:.0f} min; starting without a reservation.")
            return 0

        interval = 30 if decision.position == 0 else min(300, 60 * decision.position)
        if api.remaining < 250:
            interval = max(interval, 300)
        time.sleep(interval)


if __name__ == "__main__":
    sys.exit(main())
