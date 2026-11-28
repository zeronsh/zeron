# CI

## Pull requests and merge queue

[`pr-ci.yml`](../../.github/workflows/pr-ci.yml) runs on pull requests and
merge queue commits. The `scope` job selects checks from the changed paths.

- **Engine, CLI and services** checks the Rust workspace and exercises the
  engine, CLI, sync and updater. Changes to the edge services run checks in
  the Workers runtime. Changes to the installer run its Linux smoke test.
- **Desktop app** builds ZGX on Linux and exercises its CLI, chat actions and
  attachments with Rust integration and Luau UI tests.
- **Desktop compile check** checks the desktop app, engine and CLI on macOS
  and Windows when native code or its build inputs change. Changes to Windows
  packaging also run the installer smoke test.
- **iOS app** checks the generated bindings and builds the simulator app.
- **Android app** builds the APK and App Bundle with a debug key.

Edits to `pr-ci.yml` select all checks. **CI result** requires the
scope job and every selected check to pass.

After a successful run, require the GitHub Actions check **CI result** in
the `main` ruleset and require branches to be up to date before merging.
Update existing pull requests from `main` to start the new checks. A
maintainer may need to approve workflow runs from forks.

### CI emergency

Start [CI emergency](../../.github/workflows/ci-emergency.yml) from the
Actions tab with the pull request number. It cancels other active PR CI runs
once the selected pull request has an active run. If its latest run failed,
was cancelled or timed out, CI emergency first requests a rerun. GitHub
accepts reruns for up to 30 days. Approve any pending fork run before using
CI emergency. With `cancel-main` enabled it also cancels `main`, merge queue,
scheduled and manual CI runs. Cancelled runs need a push or a rerun.
Remove any old `ci:lock` label when moving a PR to the new workflow so the
label stops reserving runners for the old workflow.

## CI on main

[`ci.yml`](../../.github/workflows/ci.yml) runs on pushes to `main`, on its
nightly schedule, and by manual dispatch. It runs these jobs in order:

1. **plan** ([`scripts/ci/plan.py`](../../scripts/ci/plan.py)) lists the
   changed files and picks the suites the change needs. It drops the jobs that
   already passed on the same inputs (see [Reuse](#reuse)), and counts the
   runners the rest will use.
2. **admit** ([`scripts/ci/admit.py`](../../scripts/ci/admit.py)) waits until
   all of those runners are free.
3. **The suites** all start together. Each is a reusable workflow:
   `zgx-tests.yml` (macOS and Linux), `ui-tests.yml` (Linux core tests),
   `windows.yml`, `macos.yml` (macOS and iOS), `voice-tests.yml`,
   `android.yml`, `preview-tests.yml`, `linux-installer.yml` and
   `cursor-compatibility.yml`.
4. **Full CI result** passes when no selected suite failed or was cancelled.

## Which suites run

| Suite | Push to `main` |
|---|---|
| ZGX desktop app (`zgx-macos`, `zgx-linux`) | `apps/zgx` or crates it/plugin host builds changed |
| Linux core, Cursor, edge, installer | when their files change |
| iOS core (Linux) | app or mobile core changed |
| iOS simulator | with iOS core |
| Windows tests, macOS | when their files change |
| Windows packaging (`app`) | with Windows tests |
| Android APK | Android app, Rust crates, Android build scripts, or shared iOS assets changed |
| Voice (audio stack, Windows media runtime) | nightly or manual run only |

The `iOS core` paths come from the Cargo graph: every crate `zeron-mobile`
depends on. The `zgx` suite runs `scripts/ci/test-zgx.sh rust` (Rust host unit
tests), `scripts/ci/test-zgx.sh tests` (integration tests in `apps/zgx/tests/`),
and `scripts/ci/test-zgx.sh ui` (`zgx test ui` Luau UI tests) on macOS and
Linux, plus the engine's plugin tests on macOS. Release and beta packaging of
`apps/zgx` across macOS, Linux, and Windows runs in `zgx-build.yml` (called by
`release.yml` and `zgx-beta.yml`).

`Cargo.toml`, `Cargo.lock` and `rust-toolchain.toml` changes run everything
except a version bump: one that only changes `version = "..."` lines (the
workspace version and the workspace packages' versions in the lockfile) selects
nothing, and is left out of the reuse fingerprints. Changes to CI itself run
everything.

### Scheduled runs

- **Nightly** (`ci.yml`, schedule) runs every suite and job, including slow
  tests, with no result reuse. The same suites run on manual dispatch. Voice
  protocol, transport, MCP and voice-media also run in `core-tests`,
  `ios-core` and the Windows tests.
- **Nightly** (`release-profile-tests.yml`): Linux core suite in the release
  profile.
- **Weekly** (`weekly-tests.yml`): the tests no other workflow runs. That is
  every `zeron-engine` integration binary, plus the crates whose own tests have
  no other runner (term, syntax, theme, veil, visual, visual-gpu, zatex,
  notify).

### Slow tests

Pushes to `main` use nextest's `ci` profile (`scripts/ci/nextest.toml`).
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

## Reuse

After a new push, a suite runs again only if something it reads changed. That
is checked by content, not by what the push touched: a suite reruns when `main`
changed something it reads, and is skipped when the push only touched code it
doesn't read.

For each selected suite, `plan` hashes the inputs in the tree being tested
([`scripts/ci/reuse.py`](../../scripts/ci/reuse.py)). The inputs are:

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
    `../` path, for example `include_str!("../../../assets/file-icons/README.md")`
    or `CARGO_MANIFEST_DIR` joined with `../harness/tests/fixtures`. The
    sources are scanned on every run, so new references are picked up without
    a list to maintain. The plan summary lists the files found.

When a job passes, its last step saves a tiny Actions cache entry named after
the suite's hash and the job. `plan` skips a suite when all its jobs have such
an entry. A single Windows or voice job can be skipped while the suite's other
jobs run. Skipped suites reserve no runners and count as passed in **Full CI
result**. The plan summary shows what was reused. Only pushes to `main` reuse
these results, and only from earlier runs on `main`.

If a suite's sources build a path from the package directory's parent in
code (`.parent()`, `.ancestors()`, `join("..")`), the scan counts the whole
workspace as the suite's input.

Changing what goes into a fingerprint (`SUITE_INPUTS`, `COMMON`,
`SUITE_PACKAGES` in `reuse.py`) changes every fingerprint once, so the next run
reruns everything; recorded passes count again after that.

Limits:

- A path assembled from variables is not found by the scan. Write such
  references as one `../` literal, or add the path to the suite's
  `SUITE_INPUTS`.
- To run every suite again without result reuse, dispatch `ci.yml` manually.

## Admission

GitHub starts a job as soon as a runner frees up. When several runs compete,
each one gets some of its jobs going and holds those runners while its other
jobs queue behind other runs, so every run takes hours.

Admission prevents this. A run's `admit` job waits until the runners for the
whole run are free; only then do the run's jobs start. Each runner is released
when the job using it finishes. Admission is first come, first served. A run
that only needs Linux runners may still start ahead of a run waiting for macOS
runners, but never by taking runners that run is waiting for.

All the state lives in the GitHub API:

- The admit job's name carries the run's reservation, for example
  `admit [linux=4 windows=2 macos=1]`.
- Other runs read that reservation from their admit jobs.
- How much of it a run still holds comes from how many of its jobs have
  finished.

Repository variables:

| Variable | Default | |
|---|---|---|
| `CI_BUDGET_TOTAL` | 20 | Hosted runners all CI runs may hold at once: the organization's concurrency limit |
| `CI_BUDGET_MACOS` | 5 | macOS runners: the organization's macOS limit |
| `CI_BUDGET_WINDOWS` | 10 | Windows runners: half the pool, so Linux keeps the other half |
| `CI_ADMISSION` | on | `off` starts every run at once |

The scheduler bounds the reservation it reads from an admit job:

- it is capped at a full run (`MAX_RESERVATION`);
- jobs an admitted run reserved but never created stop counting 10 minutes
  after admission, except the **Full CI result** job and Android publishing, which
  start late (90 minutes);
- a waiting run that could have been admitted for 3 minutes but still waits
  (an edited admit step that never finishes) is ignored: the runs behind it
  stop waiting for it.

Admission ensures that all of a run's runners were free when it started. It
can't stop jobs it doesn't see from taking runners in the meantime: the pull
request workflow, other repositories of the organization, and release or
deploy workflows. Those can still take a free runner first.

### No deadlocks

Each `admit` job holds a runner until it completes. A waiting run counts those
runners only for the runs ahead of it. This prevents two runs from waiting for
each other.

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
push out the caches that `main` seeds. There is one cache per build: ZGX
(macOS and Linux, plus `zgx-cli` and `zgx-ghostty`), Linux core,
Linux voice, one shared by the Windows jobs, macOS native, iOS core and
simulator, and Android.

## Locally

```sh
python3 -m unittest discover -s scripts/tests -p 'test_ci_*.py'
```
