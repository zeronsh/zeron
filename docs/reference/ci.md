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

### Priority and locks

Adding a label needs the triage role or above, so GitHub decides who can use
these. A fork's author can't label their own pull request.

- **`ci:priority`**: the pull request's runs go to the front of the queue,
  behind any locked pull request. Nobody behind them may take runners they
  wait for, so the next runners to free up go to them. Running jobs are never
  stopped.
- **`ci:lock`**: while the pull request is open, admission treats a full run's
  worth of runners (10 Linux, 3 Windows, 3 macOS) as taken, whether or not
  the pull request is running anything. Its own runs skip the queue and are
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
| `CI_DEBOUNCE_SECONDS` | 60 | How long a pull request run waits before queueing. A push within that time cancels it. |
| `CI_ADMISSION` | on | `off` starts every run at once |
| `CI_PR_TIER` | fast | `full` runs everything on pull requests |
| `CI_LOCK_HOURS` | 6 | How long a `ci:lock` label holds runners |
| `CI_FORK_REUSE` | (unset) | `own`: fork pull requests also reuse their own earlier passes. Set it once the merge queue is required |

A pull request can edit its own copy of `ci.yml`, so the reservation other runs
read from its admit job is not trusted:

- it is capped at a full run (`MAX_RESERVATION`);
- jobs an admitted run reserved but never created stop counting 10 minutes
  after admission, except the `CI result` job and Android publishing, which
  start late (90 minutes);
- a waiting run that could have been admitted for 3 minutes but still waits
  (an edited admit step that never finishes) is ignored: the runs behind it
  stop waiting for it.

Admission ensures that all of a run's runners were free when it started. It
can't stop jobs it doesn't see from taking runners in the meantime: other
repositories of the organization, release and deploy workflows. Those can
still take a free runner first.

### No deadlocks

Every wait points the same way: locked runs (oldest lock first), then
`ci:priority`, then first come first served. So no two runs can wait for each
other.

- A waiting run's `admit` job holds a runner until the run is admitted. A run
  counts those runners only for the runs ahead of it, not behind it.
- A lock's reservation counts against everyone except older locks. A second
  lock waits until the first is released; the first never waits for the
  second.

`scripts/tests/test_ci_admit.py` simulates 3,000 random queues (any sizes,
priority, overlapping locks, any polling order) and checks that every run gets
admitted. The scheduler before this rule deadlocked in about 1 in 10 of them.

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

The repository has 10 GB of Actions cache. When it is full, GitHub evicts the
least recently used entries. Only `main` saves caches, so pull requests can't
push out the caches that `main` seeds. There is one cache per build: Linux
core, Linux UI, Linux voice, one shared by the Windows jobs, macOS native, iOS
core and simulator, and Android.

## Locally

```sh
python3 -m unittest discover -s scripts/tests -p 'test_ci_*.py'
```
