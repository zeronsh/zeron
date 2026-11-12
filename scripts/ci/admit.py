#!/usr/bin/env python3
"""Admission control for CI runs: a run starts only when all its runners are free.

GitHub queues every job of every run as soon as it can, so under load each run
gets a few of its jobs going and holds those runners while its other jobs wait
behind other runs' jobs: every run is slow, none finishes. Instead, the
`admit` job of .github/workflows/ci.yml runs this script, which waits until
the runners the whole run needs (its reservation, computed by plan.py) fit in
the runner budget, and only then lets the run's jobs start. The heavy release
workflows (RELEASE_WORKFLOWS: releases, betas) have an `admit` job too, and
join the same queue ahead of everyone (see Releases below).

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
    (after releases and locked ones): the runners that free up go to it
    first, since nobody behind it may take what it waits for.
  - `ci:lock` on a pull request keeps a full run's worth of runners
    (MAX_RESERVATION) out of everyone else's budget but releases' while the
    pull request is open, whether or not it is running anything; its own runs
    are admitted as soon as their runners are free, ahead of every queue but
    the releases'. It ends when the
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
CI_BUDGET_WINDOWS. Runs of workflows that aren't admitted (deploys, uploads,
the cache janitor) are seen as holding what their jobs run or queue for
(Scheduler `outside`), but runners this script cannot see (repositories that
aren't peers) can still take a free runner first, which is why admission is
"all runners were free when the run started", not a hard guarantee.

Releases (the `release` class):

  - A run of one of its repository's release workflows claims the class in its
    admit job's name ("admit [linux=5 windows=1 macos=0] release"). The claim
    only counts when the run was started by pushing a tag or by hand
    (workflow_dispatch), never by a pull request, which can edit any workflow
    file (Scheduler.verified_release). Otherwise the run is an ordinary one.
    A release workflow's runs are listed for those two events only
    (RELEASE_EVENTS): a pull request's runs of it are outside runs, seen as
    what they run, so however many there are they can't hide a release.
  - Verified releases go first, first come first served among themselves. A
    lock's standing reservation doesn't count against them, only what its
    runs hold: its runs wait behind the release, which must not wait for them.
  - Their reservation leaves out the jobs behind an environment's required
    reviewers (macOS signing): such a job counts once it is queued or running,
    never while it waits for approval. Their packaging jobs start only once
    their builds are done, hours after admission, so a verified release's
    jobs not created yet stay reserved for RELEASE_GRACE rather than
    CREATE_GRACE: its reservation is trusted, as only write access starts one.

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
# At most 4 digits (as PR_CLAIM): a name comes from another run's editable ci.yml,
# and int() of a number thousands of digits long raises (its str digits limit).
RESERVATION = re.compile(r"\b(linux|windows|macos)=(\d{1,4})\b")
# At least the largest reservation plan.py makes: every suite, on main (checked by
# test_ci_admit.py). One cap for every branch and repository sharing the queue
# (main, zeron/zgx with its zgx suite, zeronsh/zgx): this file is the same in each.
MAX_RESERVATION = {"linux": 11, "windows": 4, "macos": 4}
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
PR_CLAIM = re.compile(r"#(\d{1,4})\s*$")
RELEASE_CLAIM = re.compile(r"\]\s*release\s*$")
# Every repository's admitted CI workflow.
CI_WORKFLOW = "ci.yml"
# Each repository's release workflows: admitted as CI is, and the only ones whose runs
# may claim the release class (Scheduler.verified_release). This file is the same in
# every repository, so one table for all; CI_RELEASE_WORKFLOWS replaces a repository's
# entry (release_workflows).
RELEASE_WORKFLOWS = {
    "zeronsh/zeron": ("release.yml", "zgx-beta.yml"),
    "zeronsh/zgx": ("release.yml",),
}
# The events that start a release (a tag's push, a run by hand): a release workflow's
# runs are listed for these only, so a pull request's runs of an edited copy (a fork's,
# any number of them) can't push a real release out of the RECENT_RELEASES window.
RELEASE_EVENTS = ("push", "workflow_dispatch")
# How long a verified release's not-yet-created jobs stay reserved after admission: its
# packaging and publishing start only once its builds are done (a hosted job's limit).
RELEASE_GRACE = 6 * 3600
WORKFLOW_FILE = re.compile(r"[\w.-]+\.ya?ml")


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


def held(declared: dict[str, int], jobs: list[dict], since_admitted: float = 0,
         release: bool = False) -> dict[str, int]:
    """Runners an admitted run still holds: its reservation minus its finished jobs,
    or its jobs queued or running if that is more.

    After CREATE_GRACE, jobs not created yet count only up to LATE_JOBS; after
    UNSEEN_GRACE only jobs that exist and have not finished count. A verified
    `release` keeps its whole reservation until RELEASE_GRACE.
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
        if job["status"] == "completed":
            done[runner_of(job)] += 1
        elif job["status"] in ("queued", "in_progress"):
            active[runner_of(job)] += 1
        # (`waiting` is for an environment's approval, which holds no runner: a release's
        # signing job, left out of its reservation, counts once it's approved and queued.)
    if since_admitted > (RELEASE_GRACE if release else UNSEEN_GRACE):
        return active
    if since_admitted > CREATE_GRACE and not release:
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
    release: bool = False  # a verified release (Scheduler.verified_release): first in the queue


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
    """Queue order: verified releases, then locked runs (oldest lock first), then
    ci:priority, then everyone, each first come first served.

    Locks added in the same second go in the order of their keys as text, the
    tiebreak decide()'s lock_is_older uses: if the two orders disagreed, two
    such locks would each wait for the other.
    """
    if w.release:
        return (0, "", "", w.key)
    if w.pr in snapshot.locked:
        return (1, snapshot.lock_times.get(w.pr, ""), str(w.pr), w.key)
    return (2 if w.pr in snapshot.priority else 3, "", "", w.key)


def decide(me: int, mine: dict[str, int], snapshot: Snapshot, budget: dict[str, int]) -> Decision:
    """Whether run `me` may start.

    Every wait points forward in one order (verified releases, then locked runs
    by lock age, then ci:priority, then first come first served), so no cycle,
    and so no deadlock, can form:
      - a waiting run's admit job holds a runner until it is admitted, so a run
        counts the admit runners of the runs *ahead* of it, never of those
        behind it (they wait for it; counting them was a circular wait);
      - a lock's standing reservation counts against everyone except releases
        and the older locks: two locks never wait for each other, and a
        release never waits for a lock, whose runs queue behind it (only for
        what those runs hold, which they give back by finishing).
    """
    zero = {r: 0 for r in RESOURCES}
    full = need_of(MAX_RESERVATION)
    order = sorted(snapshot.waiters, key=lambda w: rank(w, snapshot))
    self_ = next((w for w in order if w.run_id == me), None)
    position = order.index(self_) if self_ is not None else len(order)
    ahead_of_me = order[:position]
    my_release = self_ is not None and self_.release
    my_lock = self_.pr if self_ is not None and self_.pr in snapshot.locked else None

    def lock_is_older(pr) -> bool:
        # (Keys are numbers here and `owner/repo#n` from a peer: compared as text on a tie,
        # as rank() orders them.)
        return (snapshot.lock_times.get(pr, ""), str(pr)) < (snapshot.lock_times.get(my_lock, ""), str(my_lock))

    def lock_stands(pr) -> bool:
        # Whether the lock's standing reservation counts against this run.
        return not my_release and (my_lock is None or (pr != my_lock and lock_is_older(pr)))

    lock_use = {pr: dict(zero) for pr in snapshot.locked}
    used = dict(zero)
    for h in snapshot.holding:
        if h.pr in snapshot.locked:
            lock_use[h.pr] = add(lock_use[h.pr], h.need)
        else:
            used = add(used, h.need)
    for pr in snapshot.locked:
        if lock_stands(pr):
            # A full run held for the lock, or what its runs hold if that is more.
            used = add(used, bigger(full, lock_use[pr]))
        else:
            # Our own lock, a newer one, or any for a release: only what its runs really hold.
            used = add(used, lock_use[pr])
    for w in ahead_of_me:
        if w.running:
            used["total"] += 1

    ahead = dict(zero)
    for w in ahead_of_me:
        if w.ignored:
            continue  # it does not take its turn; only its admit runner counts (above)
        if w.pr in snapshot.locked and lock_stands(w.pr):
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


def parse_workflows(text: str, repo: str | None = None) -> list[tuple[str, str]]:
    """A list of workflows, "owner/repo:a.yml,b.yml ..." (spaces or commas), as
    (repository, workflow) pairs. A bare file name is the repository's before
    it, or `repo`'s."""
    pairs = []
    for item in re.split(r"[\s,]+", text.strip()):
        name, colon, workflow = item.partition(":")
        if colon:
            repo = name if re.fullmatch(r"[\w.-]+/[\w.-]+", name) else None
            ok = re.fullmatch(r"[\w.-]+", workflow)
        else:
            workflow, ok = item, WORKFLOW_FILE.fullmatch(item)
        if repo and ok:
            pairs.append((repo, workflow))
    return pairs


def parse_peers(text: str) -> list[tuple[str, str]]:
    """CI_PEERS: "owner/repo:ci.yml ..." (spaces or commas), the other
    repositories whose admitted CI shares this organization's runner pool, as
    (repository, workflow) pairs. More of a peer's workflows may follow its CI's
    ("owner/repo:ci.yml,release.yml"); its release workflows are read anyway
    (release_workflows)."""
    return parse_workflows(text)


def release_workflows(repo: str, override: str, repos: list[str]) -> dict[str, tuple[str, ...]]:
    """The release workflows of this repository and its peers (`repos`):
    RELEASE_WORKFLOWS, each repository's replaced by CI_RELEASE_WORKFLOWS
    (`override`) when that names it. There, bare file names are this
    repository's ("release.yml zgx-beta.yml") and "owner/repo:a.yml,b.yml"
    another's."""
    found = {r: tuple(RELEASE_WORKFLOWS.get(r, ())) for r in repos}
    given: dict[str, list[str]] = {}
    for r, workflow in parse_workflows(override, repo):
        given.setdefault(r, []).append(workflow)
    found.update((r, tuple(workflows)) for r, workflows in given.items())
    return found


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
    Every admitted workflow joins the same queue: its waiting runs wait in it,
    its admitted runs hold what they hold, and the ci:priority / ci:lock
    labels of every repository read count as this repository's do. Those are,
    besides this run's own workflow (`workflow_id`):
      - `peers`: more admitted workflows, (repository, file) pairs: other
        repositories' CI (CI_PEERS), and this one's CI when this run is a
        release's;
      - `releases`: each repository's release workflows (release_workflows),
        whose runs may claim the release class (verified_release);
      - with `outside`, every other run in this repository and the peers
        (deploys, uploads, the cache janitor, a workflow before it adopts
        admission): what their jobs run or wait for counts as held. They're
        never admitted, only seen, so their queued jobs get the next runners
        and admitted runs never take them.
    Another repository's runs are read at most every OUTSIDE_REFRESH seconds,
    to spare the API quota.
    """

    REFRESH = 300  # refetch unchanged runs' jobs at least this often (seconds)
    OUTSIDE_REFRESH = 120  # ...and runs outside this workflow at most this often
    RECENT_RELEASES = 30  # a release workflow's latest runs, read whatever their status

    def __init__(self, api: Api, repo: str, workflow_id: int,
                 peers: list[tuple[str, str]] = (), outside: bool = False,
                 releases: dict[str, tuple[str, ...]] | None = None):
        self.api = api
        self.repo = repo
        self.workflow_id = workflow_id
        self.peers = list(peers)
        self.outside = outside
        self.releases = dict(releases or {})
        self.jobs: dict[int, tuple[str, float, list[dict]]] = {}  # run -> (updated_at, fetched, jobs)
        self.peer_ids: dict[tuple[str, str], int] = {}  # (repo, workflow file) -> the workflow's id
        self.listed: dict[str, tuple[float, list[dict]]] = {}  # list path -> (fetched, runs)
        self.tags: dict[int, bool] = {}  # push run -> whether it pushed a tag (verified_release)

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

    def list_runs(self, path: str, stale_ok: float = 0, recent: int = 0) -> list[dict]:
        """Every run `path` lists, or with `recent` only that many of the latest."""
        cached = self.listed.get(path)
        if cached and time.time() - cached[0] < stale_ok:
            return cached[1]
        if recent:
            runs = self.api.get(f"{path}&per_page={recent}")["workflow_runs"]
            self.listed[path] = (time.time(), runs)
            return runs
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
        if (repo, workflow) not in self.peer_ids:
            try:
                self.peer_ids[repo, workflow] = int(self.api.get(f"/repos/{repo}/actions/workflows/{workflow}")["id"])
            except (OSError, KeyError, ValueError, TypeError):
                return None  # not there (yet): its runs count as outside ones
        return self.peer_ids[repo, workflow]

    def admitted(self) -> dict[tuple[str, int], tuple[float, bool]]:
        """Every admitted workflow, (repo, id) -> (how stale its runs may be, whether a release workflow)."""
        admitted = {(self.repo, self.workflow_id): (0.0, False)}
        wanted = [(repo, workflow, False) for repo, workflow in self.peers]
        wanted += [(repo, workflow, True) for repo, workflows in self.releases.items() for workflow in workflows]
        for repo, workflow, release in wanted:
            workflow_id = self.peer_workflow(repo, workflow)
            if workflow_id is not None:
                default = (0.0 if repo == self.repo else self.OUTSIDE_REFRESH, False)
                stale_ok, known = admitted.get((repo, workflow_id), default)
                admitted[repo, workflow_id] = (stale_ok, known or release)
        return admitted

    def snapshot(self) -> Snapshot:
        admitted = self.admitted()
        repos = list(dict.fromkeys([self.repo] + [repo for repo, _ in self.peers] + list(self.releases)))

        lock_times: dict = {}
        priority: set = set()
        for repo in repos:
            lock_times.update(self.locks(repo))
            priority |= self.labelled(PRIORITY_LABEL, repo)
        snap = Snapshot(priority=priority, locked=set(lock_times), lock_times=lock_times)
        special = snap.priority | snap.locked

        live: set[int] = set()
        for (repo, workflow_id), (stale_ok, release_workflow) in admitted.items():
            path = f"/repos/{repo}/actions/workflows/{workflow_id}/runs"
            if release_workflow:
                # Its admit job is its first: a run waiting for a runner for it is still
                # `queued`, one waiting for an approval `waiting`. Releases are few: the latest
                # page of each RELEASE_EVENTS, whatever the status. Its other runs (a pull
                # request's) are outside ones, below. (exclude_pull_requests only trims the
                # response.)
                runs = [run for event in RELEASE_EVENTS
                        for run in self.list_runs(f"{path}?event={event}&exclude_pull_requests=true", stale_ok,
                                                  recent=self.RECENT_RELEASES)
                        if run.get("status") != "completed"]
            else:
                runs = self.list_runs(f"{path}?status=in_progress", stale_ok)
            for run in runs:
                live.add(run["id"])
                jobs = self.run_jobs(run, repo, stale_ok)
                admit = next((j for j in jobs if is_admit(j)), None)
                if admit is None:
                    if (repo != self.repo or release_workflow) and self.outside:
                        # A run without admission (a peer's CI before adopting it, a release
                        # workflow's version without it): seen as is.
                        need = outside_need(jobs)
                        if need["total"]:
                            snap.holding.append(Holding(need))
                    continue  # (this repository's CI: still planning, not asking for anything yet)
                declared = parse_reservation(admit["name"])
                if declared is None:
                    continue
                release = self.verified_release(admit["name"], run, release_workflow, repo)
                pr = None if release else self.verified_pr(admit["name"], run, special, repo)
                if admit["status"] != "completed":
                    snap.waiters.append(Waiter(run["id"], (admit["created_at"], admit["id"]),
                                               need_of(declared), admit["status"] == "in_progress", pr,
                                               release=release))
                elif admit["conclusion"] == "success":
                    since = seconds_since(admit.get("completed_at"))
                    snap.holding.append(Holding(need_of(held(declared, jobs, since, release)), pr,
                                                stalled_for(jobs)))

        if self.outside:
            release_workflows = {k for k, (_, release) in admitted.items() if release}
            for repo in repos:
                stale_ok = 0.0 if repo == self.repo else self.OUTSIDE_REFRESH
                for status in ("queued", "in_progress"):
                    for run in self.list_runs(f"/repos/{repo}/actions/runs?status={status}", stale_ok):
                        workflow = (repo, run.get("workflow_id"))
                        if run["id"] in live or (workflow in admitted and not (
                                workflow in release_workflows and run.get("event") not in RELEASE_EVENTS)):
                            continue  # (a release workflow's pull request runs: outside ones)
                        live.add(run["id"])
                        need = outside_need(self.run_jobs(run, repo, max(stale_ok, 60.0)))
                        if need["total"]:
                            snap.holding.append(Holding(need))
        self.jobs = {k: v for k, v in self.jobs.items() if k in live}
        self.tags = {k: v for k, v in self.tags.items() if k in live}
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

    def verified_release(self, admit_name: str, run: dict, release_workflow: bool, repo: str | None = None) -> bool:
        """Whether a run's release claim counts: a run of one of its repository's
        release workflows, started by hand or by pushing a tag. Never a pull
        request's, which can edit any workflow, nor a branch push's."""
        if not release_workflow or not RELEASE_CLAIM.search(admit_name):
            return False
        if run.get("event") == "workflow_dispatch":
            return True  # (dispatching needs write access)
        if run.get("event") != "push":
            return False
        if run["id"] not in self.tags:
            pushed = self.pushed_tag(run, repo or self.repo)
            if pushed is None:
                return False  # unknown for now: an ordinary run until the next poll
            self.tags[run["id"]] = pushed
        return self.tags[run["id"]]

    def pushed_tag(self, run: dict, repo: str) -> bool | None:
        """Whether a push run's ref is a tag of its commit (a run names its ref,
        `head_branch`, but not whether it's a branch or a tag); None if unknown."""
        name = run.get("head_branch") or ""
        if not name:
            return False
        try:
            target = self.api.get(f"/repos/{repo}/git/ref/tags/{quote(name)}")["object"]
            if target.get("type") == "tag":  # an annotated tag: its object names the commit
                target = self.api.get(f"/repos/{repo}/git/tags/{target['sha']}")["object"]
        except urllib.error.HTTPError as error:
            return False if error.code == 404 else None
        except (OSError, KeyError, ValueError, TypeError):
            return None
        return target.get("sha") == run.get("head_sha")


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
    # (zeronsh/zgx), every repository's release workflows, and every run admission
    # doesn't schedule: seen as holding what they run and queue. This repository's
    # CI too, for a release's admission (this run's own workflow is then the release's).
    peers = parse_peers(os.environ.get("CI_PEERS", ""))
    repos = list(dict.fromkeys([repo] + [r for r, _ in peers]))
    releases = release_workflows(repo, os.environ.get("CI_RELEASE_WORKFLOWS", ""), repos)
    scheduler = Scheduler(api, repo, workflow_id, peers=[(repo, CI_WORKFLOW)] + peers, releases=releases,
                          outside=os.environ.get("CI_SEE_OUTSIDE", "on").lower() not in ("off", "false", "0"))

    print(f"Reservation {mine}; budget {budget}; peers {repos[1:] or '-'}; release workflows {releases}")
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
                  f"priority {sorted(map(str, snap.priority)) or '-'}, locked {sorted(map(str, snap.locked)) or '-'}, "
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
