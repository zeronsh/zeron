# CI

Pull requests, `main` and the merge queue all go through one workflow,
[`ci.yml`](../../.github/workflows/ci.yml). It runs these jobs in order:

1. **plan** ([`scripts/ci/plan.py`](../../scripts/ci/plan.py)) lists the
   changed files and picks the suites the change needs. It drops the jobs that
   already passed on the same inputs (see [Reuse](#reuse)), and counts the
   runners the rest will use.
2. **admit** ([`scripts/ci/admit.py`](../../scripts/ci/admit.py)) waits until
   all of those runners are free.
3. **The suites** all start together. Each is a reusable workflow:
   `ui-tests.yml` (Linux), `windows.yml`, `macos.yml` (macOS and iOS),
   `voice-tests.yml`, `android.yml`, `preview-tests.yml`,
   `linux-installer.yml` and `cursor-compatibility.yml`.
4. **CI result** passes when no suite failed or was cancelled. This is the
   check to require in the ruleset and the merge queue.

Around it:

- [`ci-supersede.yml`](../../.github/workflows/ci-supersede.yml) stops a pull
  request's runs that a newer push or closing it made pointless (see
  [Pull request runs](#pull-request-runs)).
- [`android-play.yml`](../../.github/workflows/android-play.yml) uploads main's
  Android bundle to Google Play once CI has passed (see
  [Google Play upload](#google-play-upload)).
- [`ci-caches.yml`](../../.github/workflows/ci-caches.yml) keeps the Actions
  cache for the caches CI uses (see [Caches](#caches)).

### Pull request runs

Each pull request run has a concurrency group of its own
(`ci-pr-<number>-<run id>`), so a new push never waits for an older run.
In one group per pull request it did: the older run, cancelled, still had to
run `CI result` (`if: always()`), which queued for a runner like any job, and
the new run waited behind it ("waiting for CI #610 to complete"). `main` and
the merge queue keep one group per ref and are never cancelled.

Older runs are stopped instead:

- [`ci-supersede.yml`](../../.github/workflows/ci-supersede.yml) runs on every
  push to a pull request (`synchronize`, `reopened`) and when it closes. It
  force-cancels the pull request's CI runs that aren't on its head commit (all
  of them once it's closed), which also skips their `CI result`. It uses
  `pull_request_target`, so its token can cancel runs of a fork's pull request
  too; it never checks out the pull request's code, it only reads and cancels
  runs through the API.
- If that job is slow to get a runner, nothing waits on it: a run still
  waiting for admission checks the pull request on every poll and stops by
  itself once its commit is no longer the head, or the pull request is closed
  (`superseded` in `admit.py`, from `CI_PR` and `CI_PR_HEAD`).

### Google Play upload

[`android-play.yml`](../../.github/workflows/android-play.yml) uploads main's
signed Android bundle (`android-aab`, built by `android.yml` inside `ci.yml`)
to a Google Play testing track. It runs on `workflow_run` when CI completes
successfully on a push to `main`, and downloads the bundle from that run.

It is a workflow of its own, not a job of `ci.yml`, because the upload waits
for a reviewer (the `google-play` environment). As a job of `ci.yml`, that
wait kept the run open, and with it `main`'s concurrency group: from Nov 5 to
Nov 11 one wait held every later `main` run and nightly, which were cancelled
unstarted, and `main` saved no caches or passes all that time.

- Only the newest build waits for approval: the workflow's concurrency group
  (`android-google-play`) cancels an older run still waiting.
- A CI run that reused Android's pass built no bundle: the first job checks
  for the artifact without asking a reviewer, and the upload is skipped.
- The bundle must carry the release key: `android.yml` marks a bundle signed
  with it (`release-signed` in the artifact), and the upload refuses one
  signed with a debug key.
- Armed by `PLAY_UPLOAD_ENABLED=true`; the track and status come from
  `PLAY_TRACK` (`beta`, or `internal`) and `PLAY_RELEASE_STATUS` (`draft`, or
  `completed`). An upload by hand with other choices is `android.yml`'s
  manual run (Upload this build to Google Play).

## Which suites run

| | Pull request (fast tier) | `main`, merge queue, manual run (full tier) |
|---|---|---|
| Linux core and UI, Cursor, edge, installer | when their files change | when their files change |
| iOS core (Linux) | app or mobile core changed | app or mobile core changed |
| iOS simulator | the app, `crates/mobile` or `crates/client` changed | with iOS core |
| Windows tests, macOS | only when the change looks platform specific | when their files change |
| Windows packaging (`app`) | `apps/zeron`, `dist/windows`, packaging scripts, `crates/update` | with Windows tests |
| macOS browser / preview fixtures | browser, preview or edge code changed | always |
| Linux browser fixture | nightly or `ci:full` only | nightly or manual run only |
| Android APK | the Android app or `crates/mobile` changed | when its files change |
| Voice (audio stack, Windows media runtime) | nightly or `ci:full` only | nightly or manual run only |
| GPUI frame-source | the pinned GPUI revision moved | the same |

The `iOS core` paths come from the Cargo graph: every crate `zeron-mobile`
depends on.

On the fast tier, a change counts as platform specific when it touches:

- the platform's own files: packaging, scripts, `apps/zeron`, `crates/update`,
  and the voice crates or the mobile core for voice and Android;
- a file under `crates/` whose name says so, such as `browser/macos.rs`;
- changed code (not comments) with a platform conditional or platform API:
  `target_os`, `cfg(unix)`, `cfg(windows)`, `objc`, WebKit, `HWND`, ...

`Cargo.toml`, `Cargo.lock` and `rust-toolchain.toml` changes run everything
except a version bump: one that only changes `version = "..."` lines (the
workspace version and the workspace packages' versions in the lockfile) selects
nothing, and is left out of the reuse fingerprints. Changes to CI itself run
everything.

### The zgx interface (zeron/zgx)

`zeron/zgx` is to the zgx interface's work what `main` is to the rest
(`INTEGRATION` in `plan.py`): CI runs on its pushes too, on the pull request
tier, so each merge costs what its pull request did. Its passes and caches
serve the pull requests into it, which reuse its passes as well as `main`'s.
Its zgx caches (`zgx-tests`, the zgx CLI, Ghostty) are saved from there only.

**Only while it's protected as `main` is.** Anyone who can push to an
unprotected branch could record a pass, or save a cache, that every pull
request into it then trusts, and runs. So `plan.py` (`integration_trusted`)
reads the branch's rules on each push to it and each pull request into it
(`GET /repos/{repo}/rules/branches/zeron%2Fzgx`; readable with the plan job's
token, as the repository is public, so no extra permission) and trusts it
only if they include both `pull_request` (changes only through a pull
request) and `non_fast_forward` (no force push), or, if readable, classic
branch protection requires pull request reviews and blocks force pushes. On
any error it doesn't (fail closed). Otherwise pull requests into it reuse
`main`'s passes only, its pushes record passes nobody trusts, and its caches
aren't saved: `plan` outputs `integration-trusted`, which `ci.yml` passes to
`zgx-tests.yml`, whose
`SAVE_CACHES: ${{ github.ref == 'refs/heads/zeron/zgx' && inputs.integration-trusted }}`.

An admin has to set this up: give `zeron/zgx` `main`'s ruleset ("Protect
main": add `refs/heads/zeron/zgx` to its target branches, or a ruleset of its
own targeting it, enforcement active) with at least **Require a pull request
before merging** and **Block force pushes** (`main`'s also restricts
deletions and updates). Until then (no ruleset targets it today) its passes
and caches don't count.

A pull request's runs also restore from the pull request's own cache scope,
which its earlier commits can write (an edited workflow saves anything): a
fake zgx CLI saved there would be run by every later commit's tests. So on a
pull request, `zgx-tests.yml` first lists the entries with the key in that
scope (`actions/caches?key=…&ref=refs/pull/<n>/merge`, with `actions: read`)
and, if there is one, or the API fails, builds the CLI rather than restore
it; Ghostty's restore (any `zgx-ghostty-` entry, since it has restore keys)
likewise.

The `zgx` suite (`zgx-tests.yml`):

| Job | Runs on a pull request |
|---|---|
| `zgx-linux`: unit tests and Luau UI tests | every zgx change |
| `zgx-macos`: the same on macOS | the app's macOS code, its lockfile or patches, or a platform change in a crate it builds |
| ...with the engine's plugin tests (`engine-plugins`) | the plugin host's own code (core-tests runs them on Linux every time) |
| `zgx-windows`: unit tests, and `scripts/ci/zgx-windows-tests.txt` | the same, for Windows |

Each job is skipped on its own once it passed on the same inputs. Known
failures are in `scripts/ci/zgx-known-failures.txt`, Windows' under
`rust-windows:` (16 headless layout and path tests that fail only there).
Windows-specific regressions go in `zgx-windows-tests.txt`, not a workflow of
their own, so they run under plan and admission.

Betas and stable releases build through `zgx-build.yml` and save no GitHub
caches: a tag run can't read another tag's, and theirs evicted `main`'s.

### Scheduled runs

- **Nightly** (`ci.yml`, schedule): every suite, job and fixture, nothing
  reused, slow tests included. Voice and the Linux browser fixture run only
  here, on manual runs and with `ci:full`: in the two weeks measured, the
  voice jobs left after the move to `core-tests` never failed, and the Linux
  browser fixture's failures were window-timing flakes and package downloads.
  The rest of voice (protocol, transport, MCP, voice-media) runs in
  `core-tests`, `ios-core` and the Windows tests.
- **Nightly** (`release-profile-tests.yml`): core and UI in the release profile.
- **Weekly** (`weekly-tests.yml`): the tests no other workflow runs. That is
  every `zeron-engine` integration binary, plus the crates whose own tests have
  no other runner (term, syntax, theme, veil, visual, visual-gpu, zatex,
  notify).

### Slow tests

Pull requests and `main` use nextest's `ci` profile (`scripts/ci/nextest.toml`).
It leaves out these slow and soak tests, which the nightly runs keep:

- the doc_host sync tests at 48-chat scale or against the real 30 s quantum;
- the Codex dying-server backoff test;
- the preview leak test;
- two preview discovery tests that assert nothing happens during a sleep.

The Cursor shim stress test runs 14 rounds instead of 100
(`ZERON_STRESS_ROUNDS`).

Test binaries that run nothing on Linux are not built there:

- Windows-only files;
- files whose every test is `#[ignore]`d.

Small integration suites share one binary per crate (`tests/misc/`), since
every `tests/*.rs` file is linked against the whole crate.

To run every suite on a pull request, add the `ci:full` label, then push or
re-run the CI workflow. To make every pull request run the full tier, set the
repository variable `CI_PR_TIER=full`.

## Reuse

After a new push, a suite runs again only if something it reads changed. That
is checked by content, not by what the push touched: a suite reruns when `main`
changed something it reads, and is skipped when the push only touched code it
doesn't read.

For each selected suite, `plan` hashes the inputs in the tree being tested
([`scripts/ci/reuse.py`](../../scripts/ci/reuse.py)). For a pull request that
tree is the merge with the current `main`. The inputs are:

- `Cargo.toml`, `Cargo.lock`, the toolchain and `ci.yml`;
- the suite's workflow, the CI scripts and setup actions it runs
  (`nextest.toml`, `test-core.sh`, the apt and nextest installers…), and the
  non-Rust files it uses. The planner and the scheduler (`plan.py`,
  `admit.py`, `reuse.py`, `emergency.py`) decide what runs, not whether a test
  passes, so changing them reuses everything;
- for Rust suites:
  - the packages they test and every workspace package those depend on, of any
    dependency kind;
  - every file outside those packages that their Rust sources name with a
    `../` path, for example `include_str!("../../../ui/src/file-icons.json")`
    or `CARGO_MANIFEST_DIR` joined with `../harness/tests/fixtures`. The
    sources are scanned on every run, so new references are picked up without
    a list to maintain. The plan summary lists the files found.

When a job passes, its last step saves a tiny Actions cache entry named after
the suite's hash and the job. `plan` skips a suite when all its jobs have such
an entry. A single Windows or voice job can be skipped while the suite's other
jobs run. Skipped suites reserve no runners and count as passed in `CI result`.
The plan summary shows what was reused.

Which recorded passes count:

| Run | Reuses passes from |
|---|---|
| Pull request from a branch of this repository | its own earlier runs, and `main` |
| Pull request from a fork | `main` only (its own too with `CI_FORK_REUSE=own`) |
| `main` | `main` only |
| Merge queue (the last gate before `main`), `ci:full` label, manual run | nothing |

Reuse fails closed:

- If a pull request changes a Rust build file (`crates/`, `apps/zeron`,
  `Cargo.*`, `.cargo/`) that no selected suite's fingerprint covers, nothing is
  reused.
- If a suite's sources build a path from the package directory's parent in
  code (`.parent()`, `.ancestors()`, `join("..")`), the scan can't follow it.
  That suite then counts the whole workspace as its input.

A pull request could fake a pass in its own runs: an earlier push whose edited
workflow saves the pass entry for the fingerprint of a later, clean-looking
commit would let that commit's suites be skipped. No other ref ever sees those
entries, but until the merge queue (which reuses nothing) is the required gate,
a green `CI result` on the pull request is what a reviewer goes by. So forks
reuse only `main`'s passes; set `CI_FORK_REUSE=own` once the merge queue is
required.

Changing what goes into a fingerprint (`SUITE_INPUTS`, `COMMON`,
`SUITE_PACKAGES` in `reuse.py`) changes every fingerprint once, so the next run
reruns everything; recorded passes count again after that.

Limits:

- A path assembled from variables is not found by the scan. Write such
  references as one `../` literal, or add the path to the suite's
  `SUITE_INPUTS`.
- A test that depends on the network or on timing and passed by luck stays
  passed until its inputs change. Re-run with `ci:full` to check everything
  again.

## Admission

GitHub starts a job as soon as a runner frees up. When several runs compete,
each one gets some of its jobs going and holds those runners while its other
jobs queue behind other runs, so every run takes hours.

Admission prevents this. A run's `admit` job waits until the runners for the
whole run are free; only then do the run's jobs start. Each runner is released
when the job using it finishes. Admission is first come, first served. A run
that only needs Linux runners may still start ahead of a run waiting for macOS
runners, but never by taking runners that run is waiting for.

All the state lives in the GitHub API, so admission works with the read-only
token that pull requests from forks get:

- The admit job's name carries the run's reservation, for example
  `admit [linux=4 windows=2 macos=1]`.
- Other runs read that reservation from their admit jobs.
- How much of it a run still holds comes from how many of its jobs have
  finished.

The queue's order: [releases](#releases), then locked pull requests, then
`ci:priority`, then everyone, each first come first served.

### Releases

The heavy release workflows are admitted as CI is, and go first:
[`release.yml`](../../.github/workflows/release.yml) (a `v*` tag, or by hand),
`zgx-beta.yml` on `zeron/zgx` (a `beta-v*` tag, or by hand) and zgx's
`release.yml` (its dry run on a `v*` tag, or by hand). Before, admission only
saw what their jobs ran or queued for: their later jobs (packaging, once the
builds are done) didn't count until GitHub created them, so CI could be
admitted into the runners they were about to need, and a release queued like
anything else.

Each has an `admit` job, which every other job of the workflow needs. Its
name carries the reservation and the class, for example
`admit [linux=5 windows=1 macos=0] release`. The workflow computes the
reservation from its event, ref and inputs (zgx-beta's `platforms`): every job
it will run but the admit job, and but those waiting for a reviewer.

- **The claim is verified**, as a pull request's claim is. It counts only for a
  run of one of its repository's release workflows (`RELEASE_WORKFLOWS` in
  `admit.py`: `release.yml` and `zgx-beta.yml` here, `release.yml` in zgx;
  `CI_RELEASE_WORKFLOWS` replaces them), started by pushing a tag or by hand.
  A pull request can edit any workflow, so its run is never a release,
  whatever its admit job says; neither is a branch push. Unverified, the run
  is an ordinary one. A release workflow's runs are read from the latest 30
  of each of those two events (`push`, `workflow_dispatch`), so a fork's
  pull requests running an edited copy can't push a real release out of the
  window; their runs are seen as what they run, as outside runs are.
- **Releases go first**, first come first served among themselves. A lock's
  standing reservation doesn't count against them, only what the locked pull
  request's runs hold (see [No deadlocks](#no-deadlocks)).
- **Jobs behind a reviewer aren't reserved.** A job whose environment has
  required reviewers can wait for approval for hours: release.yml's macOS
  build on a tag or `main`, zgx-beta's `macos package` on a tag (both
  `apple-release`). It is left out of the reservation, and a job waiting for
  approval holds no runner and doesn't count. Once approved, it counts as soon
  as it's queued.
- **Later jobs stay reserved.** Only write access starts a verified release,
  so its reservation is trusted: its jobs not created yet stay reserved for 6
  hours, where CI's stop counting after 10 minutes.
- No debounce. Admission fails open as CI's does: `CI_ADMISSION=off`, API
  trouble or a 2-hour wait start the release at once. So does a crash: the
  admit step has `continue-on-error: true` (a step that fails with it has
  outcome `failure` but conclusion `success`, so its job succeeds and every
  job with `needs: admit` runs; on the step rather than the job, since
  what `needs` sees of a failed job with it isn't documented) and a timeout
  of its own, shorter than the job's (which would cancel the job), so a hang
  ends the same way.

The rest is seen, not admitted: the Google Play upload, TestFlight, deploys,
the cache janitor (`ci-caches.yml`), `ci-supersede.yml` and CI emergency.
They're small (a job or two), or most of their time is a reviewer's wait,
which doesn't fit a queue; an admit job would cost them a runner and a wait
out of proportion. What their jobs run or queue for counts as held, so
admitted runs don't take those runners (see
[Across repositories](#across-repositories)).

### Priority and locks

Adding a label needs the triage role or above, so GitHub decides who can use
these. A fork's author can't label their own pull request.

- **`ci:priority`**: the pull request's runs go to the front of the queue,
  behind releases and any locked pull request. Nobody behind them may take
  runners they wait for, so the next runners to free up go to them. Running
  jobs are never stopped.
- **`ci:lock`**: while the pull request is open, admission treats a full run's
  worth of runners (`MAX_RESERVATION`: 11 Linux, 4 Windows, 4 macOS) as taken
  for everyone but releases, whether or not the pull request is running
  anything. Its own runs skip the queue, behind releases only, and are
  admitted as soon as their runners are free. The lock ends when:
  - the label is removed;
  - the pull request is merged or closed;
  - `CI_LOCK_HOURS` (6) have passed since the label was added. Add the label
    again to extend.
- **CI emergency** ([`ci-emergency.yml`](../../.github/workflows/ci-emergency.yml)):
  a workflow you start by hand from the Actions tab with a pull request number.
  It needs write access. It:
  - adds `ci:lock` to the pull request;
  - cancels every other pull request's CI run (with `cancel-main`, also
    `main` and merge queue runs);
  - re-runs the pull request's own CI if none is running and the last run
    didn't pass.

  The cancelled pull requests need a push or a re-run afterwards.

A run states its pull request in its admit job's name, for example
`admit [linux=4 windows=0 macos=1] #1077`. The claim only counts when that
pull request's head is the run's commit, so a run can't borrow another pull
request's priority or lock.

Repository variables:

| Variable | Default | |
|---|---|---|
| `CI_BUDGET_TOTAL` | 20 | Hosted runners all CI runs may hold at once: the organization's concurrency limit |
| `CI_BUDGET_MACOS` | 5 | macOS runners: the organization's macOS limit |
| `CI_BUDGET_WINDOWS` | 10 | Windows runners: half the pool, so Linux keeps the other half |
| `CI_DEBOUNCE_SECONDS` | 60 | How long a pull request run waits before queueing. A push within that time supersedes it. |
| `CI_ADMISSION` | on | `off` starts every run at once |
| `CI_PR_TIER` | fast | `full` runs everything on pull requests |
| `CI_LOCK_HOURS` | 6 | How long a `ci:lock` label holds runners |
| `CI_FORK_REUSE` | (unset) | `own`: fork pull requests also reuse their own earlier passes. Set it once the merge queue is required |
| `CI_PEERS` | `zeronsh/zgx:ci.yml,release.yml` | Other repositories' admitted workflows sharing the runner pool: `owner/repo:ci.yml`, separated by spaces or commas, and any more of a repository's after its CI's (`,release.yml`). Their release workflows are read whether listed or not |
| `CI_RELEASE_WORKFLOWS` | (`admit.py`'s `RELEASE_WORKFLOWS`) | The release workflows, whose tag and manual runs go first: file names for this repository's (`release.yml zgx-beta.yml`), `owner/repo:a.yml,b.yml` for another's |
| `CI_SEE_OUTSIDE` | on | `off`: ignore runs of workflows that aren't admitted (deploys, uploads, the cache janitor, a peer's other workflows) |
| `PLAY_UPLOAD_ENABLED` | (unset) | `true`: `android-play.yml` uploads main's bundle to Google Play after CI passes |
| `PLAY_TRACK` | beta | Google Play testing track for that upload (`beta` or `internal`) |
| `PLAY_RELEASE_STATUS` | draft | Release status for that upload (`draft` or `completed`) |

A pull request can edit its own copy of `ci.yml`, so the reservation other runs
read from its admit job is not trusted:

- it is capped at a full run (`MAX_RESERVATION`). The cap is the largest
  reservation of any branch or repository sharing the queue (`main`,
  `zeron/zgx` with its zgx suite, `zeronsh/zgx`), since `admit.py` is the
  same file in each: 11 Linux, 4 Windows, 4 macOS. `test_ci_admit.py` checks
  this branch's largest plan fits under it, within a job per runner type;
- jobs an admitted run reserved but never created stop counting 10 minutes
  after admission, except the `CI result` job and Android publishing, which
  start late (90 minutes). A verified release's count for 6 hours;
- a waiting run that could have been admitted for 3 minutes but still waits
  (an edited admit step that never finishes) is ignored: the runs behind it
  stop waiting for it.

### Across repositories

The organization's 20 hosted runners (5 macOS) are one pool for `zeronsh/zeron`
and `zeronsh/zgx`. On Nov 10, zgx's CI alone held a median 18 of the 20 for
half a day, while admission here, seeing only this repository's CI, admitted
runs as if every runner were free; they then queued for hours. So admission
reads, besides this repository's `ci.yml` and release workflows:

- **Peers** (`CI_PEERS`, `zeronsh/zgx:ci.yml,release.yml` by default): other
  repositories' admitted workflows, their CI and their release workflows,
  join the same queue. Their waiting runs wait in it, their admitted runs
  hold what they hold, their releases go first as this repository's do, and
  their `ci:priority` and `ci:lock` labels count as this repository's do. A
  peer's pull request is keyed `owner/repo#n`, never confused with a pull
  request number here.
- **Everything else** (`CI_SEE_OUTSIDE`, on by default): every other run in
  this repository and the peers (deploys, uploads, the cache janitor, a run of
  a workflow from before it adopted admission, such as a release tagged on an
  older commit, or `main`'s `zgx-beta.yml`). What its jobs run or queue for
  counts as held, so its queued jobs get the next free runners and admitted
  runs don't take them. A job waiting for an environment's approval holds no
  runner and doesn't count.

Another repository's runs are read at most every 2 minutes, to spare the
token's API quota. The peers are public, so the run's own token reads them.

Admission ensures that all of a run's runners were free when it started. Jobs
it can't see (a repository that isn't a peer) can still take a free runner
first.

### No deadlocks

Every wait points the same way: releases, then locked runs (oldest lock
first), then `ci:priority`, then first come first served. So no two runs can
wait for each other.

- A waiting run's `admit` job holds a runner until the run is admitted. A run
  counts those runners only for the runs ahead of it, not behind it.
- A lock's reservation counts against everyone except older locks and
  releases. A second lock waits until the first is released; the first never
  waits for the second. A release never waits for a lock's reservation: the
  locked pull request's runs queue behind the release, and the lock ends only
  once the pull request merges, so the two would wait for each other until
  `CI_LOCK_HOURS`. It waits only for what the lock's runs hold, which they
  give back by finishing.

`scripts/tests/test_ci_admit.py` simulates 3,000 random queues (any sizes,
releases, priority, overlapping locks, any polling order) and checks that
every run gets admitted. The scheduler before this rule deadlocked in about 1
in 10 of them; with releases waiting for locks' reservations, it gets stuck too.

Waiting costs a runner: an `admit` job runs on the same hosted pool as the
suites. With about 20 runs waiting at once, their admit jobs took the whole
pool, admitted runs' jobs got no runner, and nothing moved until the 2-hour
timeout. So when an admitted run's Linux or Windows job has been queued for 10
minutes, every waiting admit job but the first 5 (a quarter of
`CI_BUDGET_TOTAL`) in the queue gives its runner back and starts its run
without a reservation. Others count that run as holding its runners, so
admission catches up. The test models GitHub's runner pool as well as
admission: without this rule most of its 30-run queues never drain, with it
all of them do.

Admission never blocks CI. It lets the run start anyway if:

- it waited 2 hours;
- the API keeps failing;
- the API rate limit runs out for more than 10 minutes.

The `GITHUB_TOKEN` gets 1000 to 5000 API requests an hour per repository,
depending on the plan; zeron has seen 5000. To stay well under that,
admission:

- uses conditional requests, which cost nothing when nothing changed;
- refetches a run's jobs only when the run changed;
- polls every 30 s at the head of the queue, 1 and 2 min at the next two
  places, 5 min further back, and 10 min when fewer than 400 requests are
  left. The quota is shared with every workflow (releases, the Cursor SDK
  update, CI emergency).

## Caches

The repository has GitHub's free 10 GB of Actions cache. When it's full,
GitHub evicts the least recently used entries, whoever saved them. Caches
saved from tags and side branches (`release.yml`, `zgx-beta.yml`: 5 to 9 GB a
run, never restored by anything) and from pull requests (`actions/cache`,
which saves by default, setup-node's `cache: npm`, setup-zig's own cache)
evicted the caches `main`'s CI restores on every run.

So every cache is declared, in one allowlist:
[`scripts/ci/caches.py`](../../scripts/ci/caches.py). A family is a pattern
its keys match, the refs allowed to save it, a priority and an expected size.
A lint enforces it on the workflows, and a janitor removes everything else.

### What is kept

In priority order, highest first. Value is the build minutes a cache saved
per month per GB it takes, as measured; zgx's suite, not on `main` yet, comes
after Linux core.

| Priority | Family | Key | Saved from | Size | Value |
|---|---|---|---|---|---|
| 1 | windows | `v0-rust-windows-Windows_NT-x64-…` (`windows-ci-setup`, shared key `windows`) | main | 1.25 GB | 7,640 |
| 2 | linux-ui | `ci-linux-v1-ui-Linux-x64-…` (`linux-ci`) | main | 1.15 GB | 7,200 |
| 3 | linux-core | `ci-linux-v1-core-Linux-x64-…` (`linux-ci`) | main | 0.68 GB | 6,350 |
| 4 | zgx-tests-macos | `v0-rust-zgx-tests-zgx-macos-Darwin-arm64-…` | zeron/zgx | 1.4 GB | |
| 5 | zgx-tests-linux | `v0-rust-zgx-tests-zgx-linux-Linux-x64-…` | zeron/zgx | 0.8 GB | |
| 6 | ios-tests | `v0-rust-ios-tests-Darwin-arm64-…` (`macos.yml`) | main | 0.76 GB | 3,840 |
| 7 | macos-native | `v0-rust-macos-native-Darwin-arm64-…` (`macos.yml`) | main | 1.79 GB | 3,170 |
| 8 | ios-core | `v0-rust-ios-core-Linux-x64-…` (`macos.yml`) | main | 0.81 GB | 2,950 |
| 9 | apt-ui-browser | `apt-debs-v1-ui-browser-…` (`linux-ci`) | main | 0.26 GB | |
| 10 | zgx-ghostty | `zgx-ghostty-…` | zeron/zgx | 0.49 GB | |
| 11, 12 | zgx-cli-macos, zgx-cli-linux | `zgx-cli-macOS-ARM64-…`, `zgx-cli-Linux-X64-…` | zeron/zgx | 20 MB each | |
| | pass records | `ci-pass-v1-…` (see [Reuse](#reuse)) | any ref (kept: main, zeron/zgx, pull requests) | ~240 B each | |

The keep set adds up to about 9.4 GB, so the budget is 9.5 GB of the 10:
`test_ci_caches.py` fails if the declared sizes exceed it. The zgx families
are saved only from `zeron/zgx` (zgx's suite isn't on `main` yet), through
`zgx-tests.yml`'s
`SAVE_CACHES: ${{ github.ref == 'refs/heads/zeron/zgx' && inputs.integration-trusted }}`,
and only while it's protected (see [The zgx interface](#the-zgx-interface-zeronzgx)).
Pass records are a few hundred bytes and needed wherever CI runs, pull
requests included, so any ref may save them. They don't count against the
budget (any pull request, a fork's too, can save any number of them, and they
must not push out a cache), and the janitor keeps only those of refs some run
trusts: `main`, `zeron/zgx` and pull requests' merge refs
(`refs/pull/<n>/merge`). The rest (a run by hand on a side branch, the merge
queue, a tag) nothing reads. A fork's pull request can't be told from a
same-repository one by its ref, so every pull request's records stay.

Not saved any more (each still restores whatever is there, so nothing else
changes):

- Android (0.9 GB, value 2,120), the GPUI frame-source job (270), TestFlight's
  `ios-core` (190), Linux voice and voice's `windows-runtime`, and voice's
  `.deb` directory: `save-if: false`, or no `save-apt-cache`.
- `release.yml` and `zgx-beta.yml`: their Rust caches were saved from a tag
  and never restored by anything else. `save-if: false`; zgx-beta's Ghostty
  build is restored only (`zgx-tests.yml` on `zeron/zgx` saves it), and its
  setup-zig has `use-cache: false`. Release and beta builds don't use the
  GitHub cache; stage 2 will give them, and CI, a shared compile cache on R2.
- setup-node's npm cache (`node-cache-…`), `cache: npm` removed: `npm ci` is
  quick, and the cache was saved from every ref.

### Who may save what

The lint (`scripts/tests/test_ci_caches.py`, run by `plan` in every CI run,
which reads the workflows and actions by indentation, without PyYAML) fails
when:

- a `Swatinem/rust-cache` step has no `save-if` (rust-cache saves from every
  ref by default), or one other than: `false`; main only,
  `${{ github.ref == 'refs/heads/main' }}`; zgx's
  `${{ env.SAVE_CACHES == 'true' }}`; or, in `linux-ci` and
  `windows-ci-setup`, an input and main,
  `${{ inputs.save-rust-cache == 'true' && github.ref == 'refs/heads/main' }}`;
- a step that saves (a rust-cache step that can, an `actions/cache/save`)
  saves a key no family declares, or from a ref its family doesn't allow. The
  lint works the key out as rust-cache does (prefix, shared key or key and
  job id, the runner's OS and architecture) and, through a composite action,
  from the caller's inputs. So voice's job `windows-runtime`, whose key starts
  `v0-rust-windows-runtime-`, is not the `windows` family;
- `actions/cache@` (restore and save in one) is used anywhere: use
  `actions/cache/restore`, and `actions/cache/save` with an `if:`;
- an `actions/cache/save` step's `if:` doesn't limit it to one allowed ref
  (`github.ref == 'refs/heads/main'` or `env.SAVE_CACHES == 'true'`, joined
  to the rest with `&&`, never `||`), unless it's a pass record
  (`key: ${{ inputs.pass-key }}-<job>`, `path: rust-toolchain.toml`);
- setup-node, setup-python, setup-java or setup-go has a `cache:` input
  (setup-go needs `cache: false`; setup-node v5 needs
  `package-manager-cache: false`);
- `mlugg/setup-zig` lacks `use-cache: false`.

In the composite actions, the job given `save-rust-cache: 'true'` (one per
key) saves the Rust cache, and the job given `save-apt-cache: 'true'` (one
per `apt-cache`) saves the `.deb` directory; every other job only restores.

### The janitor

[`ci-caches.yml`](../../.github/workflows/ci-caches.yml) runs every 6 hours
(and by hand, with `dry-run` to delete nothing) from `main`, since scheduled
workflows run only from the default branch. It lists every ref's entries
(`main`, `zeron/zgx`, pull requests, tags) and deletes, with
`python3 scripts/ci/caches.py janitor`:

1. entries no family declares;
2. entries saved from a ref their family doesn't allow (pull requests, tags,
   side branches); pass records only from refs no run trusts (anything but
   `main`, `zeron/zgx` and `refs/pull/<n>/merge`);
3. older generations: only the newest entry of a family per ref stays.
   rust-cache's key changes with the lockfile, so `main`'s previous cache
   would otherwise linger until GitHub evicts something;
4. while what's left is still over the 9.5 GB budget, whole families, from
   the lowest priority up. Pass records don't count against it.

The run's summary lists what it kept and deleted, with sizes. To see what it
would delete now, without deleting anything:

```sh
gh cache list -R zeronsh/zeron --limit 1000 --json id,key,ref,sizeInBytes,createdAt,lastAccessedAt > caches.json
python3 scripts/ci/caches.py janitor --from-json caches.json
```

### Adding a cache

1. Measure it: its size, and the minutes it saves on the runs that restore it.
2. Declare its family in `FAMILIES` in `scripts/ci/caches.py`: the key
   pattern, the refs that may save it (`main` unless it's a branch's suite),
   its expected size, and its priority among the others (renumber the ranks
   below it). If the declared sizes no longer fit the budget, drop or lower
   another family: the janitor would drop the lowest priority anyway.
3. Save it from that ref only: rust-cache with a `save-if` of that form, or
   `actions/cache/restore` plus an `actions/cache/save` whose `if:` names the
   ref.

Or don't save it: `save-if: false` for rust-cache, `actions/cache/restore`
alone otherwise. A cache restored from `main` costs nothing.

`python3 scripts/ci/caches.py lint` runs the lint alone, and
`python3 scripts/ci/caches.py families` prints the allowlist.

## Locally

```sh
python3 -m unittest discover -s scripts/tests -p 'test_ci_*.py'
```
