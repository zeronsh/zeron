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
| Linux core, UI and browser, Cursor, edge, installer | when their files change | when their files change |
| iOS | app or mobile core changed | app or mobile core changed |
| Windows, macOS, voice, Android | only when the change looks platform specific | when their files change |

On the fast tier, a change counts as platform specific when it touches:

- the platform's own files: packaging, scripts, `apps/zeron`, `crates/update`,
  and the voice crates or the mobile core for voice and Android;
- a file under `crates/` whose name says so, such as `browser/macos.rs`;
- changed code (not comments) with a platform conditional or platform API:
  `target_os`, `cfg(unix)`, `cfg(windows)`, `objc`, WebKit, `HWND`, ...

`Cargo.toml`, `Cargo.lock` and `rust-toolchain.toml` run everything. Changes to
CI itself also run everything.

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

- `Cargo.toml`, `Cargo.lock`, the toolchain, `ci.yml`, the composite actions
  and `scripts/ci`;
- the suite's workflow and the non-Rust files it uses;
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
| Pull request | its own earlier runs, and `main` |
| `main`, merge queue | `main` only |
| `ci:full` label, manual run | nothing |

A pull request could fake a pass in its own runs, but it could just as well
edit its own workflows. No other ref ever sees those entries.

Limits:

- A path built in pieces (`.join("..").join("ui")`) is not found by the scan.
  Write such references as one `../` literal, or add the path to the suite's
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
  worth of runners (10 Linux, 3 Windows, 4 macOS) as taken, whether or not
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
| `CI_BUDGET_TOTAL` | 50 | Hosted runners all CI runs may hold at once. Leave room for the organization's other repositories. |
| `CI_BUDGET_MACOS` | 10 | macOS runners |
| `CI_BUDGET_WINDOWS` | 16 | Windows runners |
| `CI_DEBOUNCE_SECONDS` | 60 | How long a pull request run waits before queueing. A push within that time cancels it. |
| `CI_ADMISSION` | on | `off` starts every run at once |
| `CI_PR_TIER` | fast | `full` runs everything on pull requests |
| `CI_LOCK_HOURS` | 6 | How long a `ci:lock` label holds runners |

A pull request can edit its own copy of `ci.yml`, so the reservation other runs
read from its admit job is not trusted:

- it is capped at a full run (`MAX_RESERVATION`);
- jobs an admitted run reserved but never created stop counting 90 minutes
  after admission.

Admission ensures that all of a run's runners were free when it started. It
can't stop jobs it doesn't see from taking runners in the meantime: other
repositories of the organization, release and deploy workflows. The budget
leaves headroom for those.

Admission never blocks CI. It lets the run start anyway if:

- it waited 5 hours;
- the API keeps failing;
- the API rate limit runs out for more than 10 minutes.

The `GITHUB_TOKEN` gets 1000 to 5000 API requests an hour per repository,
depending on the plan; zeron has seen 5000. To stay well under that,
admission:

- uses conditional requests, which cost nothing when nothing changed;
- refetches a run's jobs only when the run changed;
- polls less often the further back in the queue it is.

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
