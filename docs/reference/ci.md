# CI

Pull requests, `main` and the merge queue all go through one workflow,
[`ci.yml`](../../.github/workflows/ci.yml). It runs these jobs in order:

1. **plan** ([`scripts/ci/plan.py`](../../scripts/ci/plan.py)) lists the
   changed files and picks the suites the change needs. It also counts the
   runners those suites will use.
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

Repository variables:

| Variable | Default | |
|---|---|---|
| `CI_BUDGET_TOTAL` | 50 | Hosted runners all CI runs may hold at once. Leave room for the organization's other repositories. |
| `CI_BUDGET_MACOS` | 10 | macOS runners |
| `CI_BUDGET_WINDOWS` | 16 | Windows runners |
| `CI_DEBOUNCE_SECONDS` | 60 | How long a pull request run waits before queueing. A push within that time cancels it. |
| `CI_ADMISSION` | on | `off` starts every run at once |
| `CI_PR_TIER` | fast | `full` runs everything on pull requests |

Admission ensures that all of a run's runners were free when it started. It
can't stop jobs it doesn't see from taking runners in the meantime: other
repositories of the organization, release and deploy workflows. The budget
leaves headroom for those.

Admission never blocks CI. It lets the run start anyway if:

- it waited 5 hours;
- the API keeps failing;
- the API rate limit runs out for more than 10 minutes.

The `GITHUB_TOKEN` gets 1000 API requests an hour per repository. To stay
under that, admission:

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
