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
another run is not trusted:

  - it is capped at the most plan.py ever reserves (MAX_RESERVATION);
  - an admitted run's jobs that have not shown up CREATE_GRACE after admission
    stop counting, except the LATE_JOBS that start after the others (the
    result job, Android publishing), which stop at UNSEEN_GRACE;
  - a waiting run that could have been admitted for UNRESPONSIVE_GRACE but
    still waits is not following this protocol (an edited admit step that
    never finishes): the runs behind it stop waiting for it.

So a run cannot hold more than a full run's worth of runners, hold runners it
never uses for long, or block the queue by never taking its turn.

Waiting costs a runner: the admit job runs on the same hosted pool as the
suites. When so many runs wait at once that their admit jobs leave admitted
runs' jobs queued (STALL_SECONDS without a runner), the runs furthest back
yield: all running admit jobs but the first quarter of the budget
(KEEP_WAITING_SHARE) stop waiting and start their suites without a
reservation, which frees their runners. Without this,
about 20 runs waiting at once took the whole pool and nothing ran until the
MAX_WAIT_MINUTES timeout.

Priority and locks (labels; adding a label needs the triage role or above):

  - `ci:priority` on a pull request puts its runs at the front of the queue
    (after locked ones): the runners that free up go to it first, since nobody
    behind it may take what it waits for.
  - `ci:lock` on a pull request keeps a full run's worth of runners
    (MAX_RESERVATION) out of everyone else's budget while the pull request is
    open, whether or not it is running anything; its own runs are admitted as
    soon as their runners are free, ahead of every queue. It ends when the
    label is removed, the pull request is merged or closed, or LOCK_HOURS
    after the label was added (add it again to extend).
  - A run claims its pull request in its admit job's name ("... #123"); the
    claim only counts when that pull request's head is the run's commit.

The emergency workflow (.github/workflows/ci-emergency.yml, write access)
adds `ci:lock` and cancels the other pull requests' CI runs.

A waiting run is admitted when, for every resource it needs, what admitted
runs hold + what the runs ahead of it in the queue need + its own need fits in
the budget. So a run that needs only Linux can go ahead of a run waiting for
a macOS runner, but never takes anything that run is waiting for (conservative
backfilling: nobody is overtaken on a resource they wait for).

Resources: `total` (every hosted job, the org-wide concurrency limit), and
`macos` and `windows`. Budgets default to the organization's runner limits
(20 jobs, 5 on macOS; Windows capped at 10 so Linux keeps half), overridable
with the repository variables CI_BUDGET_TOTAL / CI_BUDGET_MACOS /
CI_BUDGET_WINDOWS. Runners this script cannot see (other repositories of the
organization, release and deploy workflows) can still take a free runner
first, which is why admission is "all runners were free when the run
started", not a hard guarantee.

The GITHUB_TOKEN may make 1000 API requests an hour per repository, shared by
every job. The script uses conditional requests (a 304 is free), refetches a
run's jobs only when the run changed, polls less often the further back in the
queue it is, and slows down when the remaining quota is low. If it cannot
reach the API, the quota resets more than 10 minutes away, or it waited
MAX_WAIT_MINUTES (120), it lets the run start (fail open) rather than block CI.
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
from urllib.parse import quote
from dataclasses import dataclass, field

RUNNERS = ("linux", "windows", "macos")
RESOURCES = ("total", "macos", "windows")
ADMIT_PREFIX = "admit"
UNRESERVED_JOBS = ("plan",)  # jobs that run before admission and are not in reservations
RESERVATION = re.compile(r"\b(linux|windows|macos)=(\d+)\b")
# The largest reservation plan.py makes: every suite, on main (checked by test_ci_admit.py).
MAX_RESERVATION = {"linux": 10, "windows": 3, "macos": 3}
# How long an admitted run's not-yet-created jobs stay reserved. Its suites' jobs
# are created as soon as admit finishes; LATE_JOBS (the `CI result` job, and
# Android publishing on main) only once the jobs they need are done.
CREATE_GRACE = 10 * 60
UNSEEN_GRACE = 90 * 60
LATE_JOBS = {"linux": 2, "windows": 0, "macos": 0}
# A waiting run that could have been admitted this long, but still waits, is ignored.
UNRESPONSIVE_GRACE = 3 * 60
# An admitted run's job queued this long (Linux or Windows: the runners waiting
# admit jobs take from) means waiting admit jobs are starving the pool.
STALL_SECONDS = 10 * 60
# ...then all but this share of the total budget of running admit jobs (the first
# in the queue) stop waiting (should_yield).
KEEP_WAITING_SHARE = 4
PRIORITY_LABEL = "ci:priority"
LOCK_LABEL = "ci:lock"
LOCK_HOURS = 6
PR_CLAIM = re.compile(r"#(\d+)\s*$")


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

    After CREATE_GRACE, jobs not created yet count only up to LATE_JOBS; after
    UNSEEN_GRACE only jobs that exist and have not finished count.
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
    if since_admitted > CREATE_GRACE:
        return {r: active[r] + min(max(declared[r] - done[r] - active[r], 0), LATE_JOBS[r]) for r in RUNNERS}
    return {r: max(declared[r] - done[r], active[r], 0) for r in RUNNERS}


def stalled_for(jobs: list[dict]) -> float:
    """How long the run's longest-queued Linux or Windows job has waited for a runner."""
    ages = [seconds_since(job.get("created_at")) for job in jobs
            if job.get("status") == "queued" and not is_admit(job) and job["name"] not in UNRESERVED_JOBS
            and runner_of(job) != "macos"]  # macOS queues behind its own limit, not admit jobs
    return max(ages, default=0)


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
    pr: int | None = None  # its pull request, when verified
    ignored: bool = False  # does not take its turn (mark_unresponsive): nobody waits for it


@dataclass
class Holding:
    need: dict[str, int]  # need_of(held()) of an admitted run
    pr: int | None = None
    stalled: float = 0  # seconds its longest-queued job has waited for a runner (stalled_for)


@dataclass
class Snapshot:
    waiters: list[Waiter] = field(default_factory=list)
    holding: list[Holding] = field(default_factory=list)
    priority: set[int] = field(default_factory=set)  # pull requests labelled ci:priority
    locked: set[int] = field(default_factory=set)  # pull requests holding a ci:lock
    lock_times: dict[int, str] = field(default_factory=dict)  # when each lock's label was added


@dataclass
class Decision:
    admit: bool
    position: int
    used: dict[str, int]
    ahead: dict[str, int]


def add(a: dict[str, int], b: dict[str, int], sign: int = 1) -> dict[str, int]:
    return {r: a[r] + sign * b[r] for r in RESOURCES}


def bigger(a: dict[str, int], b: dict[str, int]) -> dict[str, int]:
    return {r: max(a[r], b[r]) for r in RESOURCES}


def rank(w: Waiter, snapshot: Snapshot) -> tuple:
    """Queue order: locked runs (oldest lock first), then ci:priority, then everyone, each first come first served."""
    if w.pr in snapshot.locked:
        return (0, snapshot.lock_times.get(w.pr, ""), w.key)
    return (1 if w.pr in snapshot.priority else 2, "", w.key)


def decide(me: int, mine: dict[str, int], snapshot: Snapshot, budget: dict[str, int]) -> Decision:
    """Whether run `me` may start.

    Every wait points forward in one order (locked runs by lock age, then
    ci:priority, then first come first served), so no cycle, and so no deadlock,
    can form:
      - a waiting run's admit job holds a runner until it is admitted, so a run
        counts the admit runners of the runs *ahead* of it, never of those
        behind it (they wait for it; counting them was a circular wait);
      - a lock's standing reservation counts against everyone except the
        older locks: two locks never wait for each other.
    """
    zero = {r: 0 for r in RESOURCES}
    full = need_of(MAX_RESERVATION)
    order = sorted(snapshot.waiters, key=lambda w: rank(w, snapshot))
    self_ = next((w for w in order if w.run_id == me), None)
    position = order.index(self_) if self_ is not None else len(order)
    ahead_of_me = order[:position]
    my_lock = self_.pr if self_ is not None and self_.pr in snapshot.locked else None

    def lock_is_older(pr) -> bool:
        # (Keys are numbers here and `owner/repo#n` from a peer: compared as text on a tie.)
        return (snapshot.lock_times.get(pr, ""), str(pr)) < (snapshot.lock_times.get(my_lock, ""), str(my_lock))

    lock_use = {pr: dict(zero) for pr in snapshot.locked}
    used = dict(zero)
    for h in snapshot.holding:
        if h.pr in snapshot.locked:
            lock_use[h.pr] = add(lock_use[h.pr], h.need)
        else:
            used = add(used, h.need)
    for pr in snapshot.locked:
        if my_lock is None or (pr != my_lock and lock_is_older(pr)):
            # A full run held for the lock, or what its runs hold if that is more.
            used = add(used, bigger(full, lock_use[pr]))
        else:
            # Our own lock, or a newer one: only what its runs really hold.
            used = add(used, lock_use[pr])
    for w in ahead_of_me:
        if w.running:
            used["total"] += 1

    ahead = dict(zero)
    for w in ahead_of_me:
        if w.ignored:
            continue  # it does not take its turn; only its admit runner counts (above)
        if w.pr in snapshot.locked and (my_lock is None or lock_is_older(w.pr)):
            continue  # its need is already in `used` as the lock's standing reservation
        ahead = add(ahead, w.need)

    # A run larger than the budget goes when nothing else holds or waits for that resource.
    fits = all(used[r] + ahead[r] + min(mine[r], budget[r]) <= budget[r]
               for r in RESOURCES if mine[r] > 0)
    return Decision(fits, position, used, ahead)


def mark_unresponsive(me: int, snapshot: Snapshot, budget: dict[str, int], since: dict[int, float],
                      now: float, grace: float = UNRESPONSIVE_GRACE) -> None:
    """Ignore waiting runs that could have been admitted for `grace` but still wait.

    An honest admit job takes its turn within a poll (30 s at the head of the
    queue). One that does not, such as an edited ci.yml whose admit step never
    finishes, would otherwise hold everyone behind it until MAX_WAIT_MINUTES.
    `since` (run -> when it was first seen admissible) persists between polls.
    Ignoring a run only lowers what others wait for, so it cannot add a wait cycle.
    """
    for w in sorted(snapshot.waiters, key=lambda w: rank(w, snapshot)):
        if w.run_id == me or not w.running:
            continue  # a queued admit job cannot take its turn yet
        if decide(w.run_id, w.need, snapshot, budget).admit:
            first = since.setdefault(w.run_id, now)
            w.ignored = now - first > grace
        else:
            since.pop(w.run_id, None)
    live = {w.run_id for w in snapshot.waiters}
    for run in [r for r in since if r not in live]:
        del since[run]


def should_yield(me: int, snapshot: Snapshot, budget: dict[str, int],
                 stall: float = STALL_SECONDS) -> bool:
    """Whether this waiting run should give its admit runner back and start now.

    Waiting admit jobs hold hosted runners from the same pool as the suites.
    When an admitted run's jobs have queued for `stall` seconds, those runners
    are what they wait for: every running admit job but the first
    budget/KEEP_WAITING_SHARE in the queue stops waiting. Its suites start
    without a reservation (others count them as held, so admission catches up),
    and its runner goes to the queued jobs, which GitHub created before the
    yielding run's.
    """
    if not any(h.stalled > stall for h in snapshot.holding):
        return False
    keep = max(2, budget["total"] // KEEP_WAITING_SHARE)
    running = [w.run_id for w in sorted(snapshot.waiters, key=lambda w: rank(w, snapshot)) if w.running]
    return me in running and running.index(me) >= keep


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


def parse_peers(text: str) -> list[tuple[str, str]]:
    """CI_PEERS: "owner/repo:workflow.yml ..." (spaces or commas), the other
    repositories whose admitted CI shares this organization's runner pool."""
    peers = []
    for item in re.split(r"[\s,]+", text.strip()):
        repo, _, workflow = item.partition(":")
        if re.fullmatch(r"[\w.-]+/[\w.-]+", repo) and workflow:
            peers.append((repo, workflow))
    return peers


def outside_need(jobs: list[dict]) -> dict[str, int]:
    """What a run admission doesn't schedule holds or is about to take: its jobs
    running or queued for a runner. (`waiting` is an environment's approval or a
    timer, which holds no runner.)"""
    counts = {r: 0 for r in RUNNERS}
    for job in jobs:
        if job.get("status") in ("queued", "in_progress"):
            counts[runner_of(job)] += 1
    return need_of(counts)


class Scheduler:
    """Builds Snapshots, refetching a run's jobs only when the run changed.

    The organization's hosted runners are one pool for all its repositories.
    Besides this repository's admitted CI workflow (`workflow_id`), it reads:
      - `peers`: other repositories' admitted CI workflows (CI_PEERS), which
        join the same queue: their waiting runs wait in it, their admitted
        runs hold what they hold, and their ci:priority / ci:lock labels
        count as this repository's do;
      - with `outside`, every other run in this repository and the peers
        (releases, betas, deploys, a peer's CI before it adopts admission):
        what their jobs run or wait for counts as held. They're never
        admitted, only seen, so a release's queued jobs get the next runners
        and admitted CI never takes them.
    Another repository's runs are read at most every OUTSIDE_REFRESH seconds,
    to spare the API quota.
    """

    REFRESH = 300  # refetch unchanged runs' jobs at least this often (seconds)
    OUTSIDE_REFRESH = 120  # ...and runs outside this workflow at most this often

    def __init__(self, api: Api, repo: str, workflow_id: int,
                 peers: list[tuple[str, str]] = (), outside: bool = False):
        self.api = api
        self.repo = repo
        self.workflow_id = workflow_id
        self.peers = list(peers)
        self.outside = outside
        self.jobs: dict[int, tuple[str, float, list[dict]]] = {}  # run -> (updated_at, fetched, jobs)
        self.peer_ids: dict[str, int] = {}  # peer repo -> its CI workflow's id
        self.listed: dict[str, tuple[float, list[dict]]] = {}  # list path -> (fetched, runs)

    def key(self, repo: str, number: int):
        """A pull request's key: its number in this repository, `owner/repo#n` in a peer."""
        return number if repo == self.repo else f"{repo}#{number}"

    def run_jobs(self, run: dict, repo: str | None = None, stale_ok: float = 0) -> list[dict]:
        repo = repo or self.repo
        cached = self.jobs.get(run["id"])
        now = time.time()
        if cached and ((cached[0] == run["updated_at"] and now - cached[1] < self.REFRESH)
                       or now - cached[1] < stale_ok):
            return cached[2]
        jobs: list[dict] = []
        page = 1
        while True:
            data = self.api.get(f"/repos/{repo}/actions/runs/{run['id']}/jobs"
                                f"?filter=latest&per_page=100&page={page}")
            jobs.extend(data["jobs"])
            if len(data["jobs"]) < 100:
                break
            page += 1
        self.jobs[run["id"]] = (run["updated_at"], now, jobs)
        return jobs

    def list_runs(self, path: str, stale_ok: float = 0) -> list[dict]:
        cached = self.listed.get(path)
        if cached and time.time() - cached[0] < stale_ok:
            return cached[1]
        runs: list[dict] = []
        page = 1
        while True:
            data = self.api.get(f"{path}&per_page=100&page={page}")
            runs.extend(data["workflow_runs"])
            if len(data["workflow_runs"]) < 100:
                break
            page += 1
        self.listed[path] = (time.time(), runs)
        return runs

    def peer_workflow(self, repo: str, workflow: str) -> int | None:
        if repo not in self.peer_ids:
            try:
                self.peer_ids[repo] = int(self.api.get(f"/repos/{repo}/actions/workflows/{workflow}")["id"])
            except (OSError, KeyError, ValueError, TypeError):
                return None  # not there (yet): its runs count as outside ones
        return self.peer_ids[repo]

    def snapshot(self) -> Snapshot:
        admitted = [(self.repo, self.workflow_id, 0.0)]
        for repo, workflow in self.peers:
            workflow_id = self.peer_workflow(repo, workflow)
            if workflow_id is not None:
                admitted.append((repo, workflow_id, self.OUTSIDE_REFRESH))
        repos = [self.repo] + [repo for repo, _ in self.peers]

        lock_times: dict = {}
        priority: set = set()
        for repo in repos:
            lock_times.update(self.locks(repo))
            priority |= self.labelled(PRIORITY_LABEL, repo)
        snap = Snapshot(priority=priority, locked=set(lock_times), lock_times=lock_times)
        special = snap.priority | snap.locked

        live: set[int] = set()
        for repo, workflow_id, stale_ok in admitted:
            runs = self.list_runs(f"/repos/{repo}/actions/workflows/{workflow_id}/runs?status=in_progress", stale_ok)
            for run in runs:
                live.add(run["id"])
                jobs = self.run_jobs(run, repo, stale_ok)
                admit = next((j for j in jobs if is_admit(j)), None)
                if admit is None:
                    if repo != self.repo and self.outside:
                        # A peer's run without admission (its CI before adopting it): seen as is.
                        snap.holding.append(Holding(outside_need(jobs)))
                    continue  # still planning: not asking for anything yet
                declared = parse_reservation(admit["name"])
                if declared is None:
                    continue
                pr = self.verified_pr(admit["name"], run, special, repo)
                if admit["status"] != "completed":
                    snap.waiters.append(Waiter(run["id"], (admit["created_at"], admit["id"]),
                                               need_of(declared), admit["status"] == "in_progress", pr))
                elif admit["conclusion"] == "success":
                    since = seconds_since(admit.get("completed_at"))
                    snap.holding.append(Holding(need_of(held(declared, jobs, since)), pr, stalled_for(jobs)))

        if self.outside:
            mine = {(repo, workflow_id) for repo, workflow_id, _ in admitted}
            for repo in repos:
                stale_ok = 0.0 if repo == self.repo else self.OUTSIDE_REFRESH
                for status in ("queued", "in_progress"):
                    for run in self.list_runs(f"/repos/{repo}/actions/runs?status={status}", stale_ok):
                        if (repo, run.get("workflow_id")) in mine or run["id"] in live:
                            continue
                        live.add(run["id"])
                        need = outside_need(self.run_jobs(run, repo, max(stale_ok, 60.0)))
                        if need["total"]:
                            snap.holding.append(Holding(need))
        self.jobs = {k: v for k, v in self.jobs.items() if k in live}
        return snap

    def labelled(self, label: str, repo: str | None = None) -> set:
        """Open pull requests carrying `label` (keys: `key`)."""
        repo = repo or self.repo
        found: set = set()
        page = 1
        while True:
            issues = self.api.get(f"/repos/{repo}/issues?state=open&labels={quote(label)}"
                                  f"&per_page=100&page={page}")
            found |= {self.key(repo, i["number"]) for i in issues if "pull_request" in i}
            if len(issues) < 100:
                return found
            page += 1

    def locks(self, repo: str | None = None) -> dict:
        """Pull requests labelled ci:lock less than LOCK_HOURS ago, and when."""
        repo = repo or self.repo
        hours = float(os.environ.get("CI_LOCK_HOURS") or LOCK_HOURS)
        locked: dict = {}
        for key in self.labelled(LOCK_LABEL, repo):
            number = key if isinstance(key, int) else int(str(key).rsplit("#", 1)[1])
            added = None
            page = 1
            while True:
                events = self.api.get(f"/repos/{repo}/issues/{number}/events?per_page=100&page={page}")
                for event in events:
                    if event.get("event") == "labeled" and event.get("label", {}).get("name") == LOCK_LABEL:
                        added = event["created_at"]
                if len(events) < 100:
                    break
                page += 1
            if added and seconds_since(added) < hours * 3600:
                locked[key] = added
        return locked

    def verified_pr(self, admit_name: str, run: dict, special: set, repo: str | None = None):
        """The pull request a run claims, if it matters and the run is really its head."""
        repo = repo or self.repo
        match = PR_CLAIM.search(admit_name)
        if not match or run.get("event") != "pull_request":
            return None
        number = int(match.group(1))
        key = self.key(repo, number)
        if key not in special:
            return None
        pull = self.api.get(f"/repos/{repo}/pulls/{number}")
        return key if pull.get("head", {}).get("sha") == run.get("head_sha") else None


def superseded(api: "Api", repo: str) -> bool:
    """Whether this pull request run is moot: the pull request was closed, or a
    newer push moved its head off the commit this run tests (CI_PR, CI_PR_HEAD).

    ci-supersede.yml force-cancels such runs, but its job queues for a runner
    like any other; this stops a waiting run without one. (Each run is in a
    concurrency group of its own: the next push's doesn't cancel it. ci.yml.)
    """
    pr, head = os.environ.get("CI_PR", "").strip(), os.environ.get("CI_PR_HEAD", "").strip()
    if not pr.isdigit() or not head:
        return False
    try:
        pull = api.get(f"/repos/{repo}/pulls/{pr}")
    except (OSError, RateLimited, KeyError, ValueError):
        return False
    return pull.get("state") == "closed" or pull.get("head", {}).get("sha") not in (None, head)


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
        # The organization's hosted-runner concurrency limit (20 jobs, 5 on macOS).
        "total": env_int("CI_BUDGET_TOTAL", 20),
        "macos": env_int("CI_BUDGET_MACOS", 5),
        "windows": env_int("CI_BUDGET_WINDOWS", 10),
    }
    max_wait = env_int("MAX_WAIT_MINUTES", 120) * 60
    debounce = env_int("CI_DEBOUNCE_SECONDS", 60) if os.environ.get("GITHUB_EVENT_NAME") == "pull_request" else 0

    repo = os.environ["GITHUB_REPOSITORY"]
    me = int(os.environ["GITHUB_RUN_ID"])
    api = Api(os.environ["GITHUB_TOKEN"], os.environ.get("GITHUB_API_URL", "https://api.github.com"))
    started = time.time()

    if debounce:
        # A newer push to the same pull request supersedes this run (ci-supersede.yml cancels it,
        # and `superseded` below stops it): pushes in quick succession only start the last one.
        print(f"Waiting {debounce}s for further pushes")
        time.sleep(debounce)

    try:
        workflow_id = api.get(f"/repos/{repo}/actions/runs/{me}")["workflow_id"]
    except (OSError, RateLimited, KeyError, ValueError) as error:
        print(f"::warning::admission unavailable ({error}); starting at once")
        return 0
    # The organization's other repositories with admitted CI on the same runners
    # (zeronsh/zgx), and every run admission doesn't schedule: seen as holding what
    # they run and queue.
    peers = parse_peers(os.environ.get("CI_PEERS", ""))
    scheduler = Scheduler(api, repo, workflow_id, peers=peers,
                          outside=os.environ.get("CI_SEE_OUTSIDE", "on").lower() not in ("off", "false", "0"))

    print(f"Reservation {mine}; budget {budget}; peers {[p for p, _ in peers] or '-'}")
    failures = 0
    last = None
    admissible_since: dict[int, float] = {}  # mark_unresponsive's memory
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

        if superseded(api, repo):
            summary("::error::A newer push to the pull request (or its closing) superseded this run: "
                    "not starting it.")
            return 1
        mark_unresponsive(me, snap, budget, admissible_since, time.time())
        decision = decide(me, mine, snap, budget)
        ignored = sorted(w.run_id for w in snap.waiters if w.ignored)
        state = (decision.position, tuple(decision.used.values()), tuple(decision.ahead.values()), tuple(ignored))
        if state != last:
            print(f"[{waited / 60:5.1f} min] position {decision.position}, in use {decision.used}, "
                  f"needed ahead {decision.ahead}, {len(snap.waiters)} waiting, "
                  f"priority {sorted(snap.priority) or '-'}, locked {sorted(snap.locked) or '-'}, "
                  f"ignored (not taking their turn) {ignored or '-'}, API quota left {api.remaining}")
            last = state
        if decision.admit:
            summary(f"Admitted after {waited / 60:.1f} min (reservation `{os.environ['CI_RESERVATION']}`, "
                    f"in use {decision.used}, budget {budget}, {api.requests} API requests).")
            return 0
        if should_yield(me, snap, budget):
            summary(f"::notice::Admitted runs' jobs are queued behind waiting admit jobs; after "
                    f"{waited / 60:.1f} min this run gives its runner back and starts without a reservation.")
            return 0
        if waited > max_wait:
            summary(f"::warning::Admission waited {waited / 60:.0f} min; starting without a reservation.")
            return 0

        # Every poll lists the runs and refetches the jobs of those that changed, from
        # the repository's shared GITHUB_TOKEN quota: only the head of the queue polls often.
        interval = {0: 30, 1: 60, 2: 120}.get(decision.position, 300)
        if api.remaining < 400:
            interval = max(interval, 600)
        time.sleep(interval)


if __name__ == "__main__":
    sys.exit(main())
