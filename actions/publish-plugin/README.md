# Publish a Zeron plugin

Submits a Zeron plugin to the store for review from a GitHub Actions job,
signed with the plugin's deploy key. It installs Zeron's Linux command line
(checked against the release's sha256) and runs
`zeron plugin publish <folder>`. No Zeron runs and nobody signs in.

## Set up

1. Publish the plugin's first version from your own machine, with Zeron
   running and signed in (`zeron plugin publish <plugin-id>`, or Settings →
   Plugins → Yours → Publish…). That makes the id yours in the store. A deploy
   key is made for a plugin you already have there.
2. Make a deploy key for the plugin, on your own machine with Zeron
   running: Settings → Plugins → Yours → the plugin → Deploy keys, or

   ```sh
   zeron plugin key create <plugin-id> --name "GitHub Actions"
   ```

   Its secret (`zpk_…`) is shown once. Zeron keeps no copy.
3. Add the secret to the repository: Settings → Secrets and variables →
   Actions → New repository secret, named `ZERON_DEPLOY_KEY`.
4. Add the step to a workflow, pinned to a Zeron release tag (or a commit
   SHA), never a branch, so the step that holds your key doesn't change
   under you:

   ```yaml
   - uses: zeronsh/zeron/actions/publish-plugin@v0.4.0
     with:
       key: ${{ secrets.ZERON_DEPLOY_KEY }}
   ```

A deploy key can submit a new version of its plugin and read its review
state, nothing else: yanking, withdrawing and managing keys are yours, from
your machine. If one leaks, the most it can do is add a version that waits
for review; revoke it (`zeron plugin key revoke <plugin-id> <key>`).

## Runners

Linux only: GitHub's `ubuntu-*` runners (x86_64 and arm64), or a
self-hosted Linux runner with `curl`, `jq`, `sha256sum` and `tar`. macOS and
Windows runners aren't supported.

## A whole workflow

To publish each time you push a tag:

```yaml
name: Publish plugin
on:
  push:
    tags: ["v*"]
jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: zeronsh/zeron/actions/publish-plugin@v0.4.0
        with:
          key: ${{ secrets.ZERON_DEPLOY_KEY }}
```

Bump `version` in `zeron-plugin.json` (and add its section to
`CHANGELOG.md`) before you tag: the store takes each version once.

## Inputs

| input | default | |
|---|---|---|
| `key` | none | The deploy key's secret. Without it the plugin is only checked, so a pull request from a fork, which gets no secrets, still runs the store's checks. |
| `path` | `.` | The plugin's folder: the one with `zeron-plugin.json`. |
| `dry-run` | `false` | Check, sign and pack the plugin, but don't submit it. |
| `zeron-version` | `latest` | The Zeron release whose command line runs. `latest` follows each new release; a version (`0.4.0`) runs that release, and the step fails if its manifest names another. |
| `sha256` | none | The sha256 of that release's command line tarball for the runner (`zeron-<version>-linux-x86_64.tar.gz` or `-aarch64`, from the release's `manifest-<version>.json`). With a pinned `zeron-version`, the step runs exactly that command line or fails. |

The action's tag pins the script; the command line it runs is fetched from
Zeron's releases, checked against the release's sha256. With
`zeron-version: latest` that is whichever release is newest when the job
runs. To hold the step still, the command line included, pin both:

```yaml
- uses: zeronsh/zeron/actions/publish-plugin@v0.4.0
  with:
    key: ${{ secrets.ZERON_DEPLOY_KEY }}
    zeron-version: 0.4.0
    sha256: <the tarball's sha256 for your runner>
```

## When it fails

The job's log says why, one line per problem, and the step exits with:

| code | meaning |
|---|---|
| 1 | The plugin doesn't meet the store's rules (a missing README, a version that isn't strict semver or was submitted already, …). Fix it and push again. |
| 2 | Set-up: no plugin in that folder, the secret is damaged, or the runner's clock is more than five minutes out. |
| 3 | The store refused the key or the account: the key was revoked, has expired or is another plugin's, the account has no username yet, or the id is someone else's. |
| 4 | The store couldn't be reached, or what it received was damaged on the way. Re-run the job. |

Revoke a key you no longer use, or one that may have leaked, with
`zeron plugin key revoke <plugin-id> <key>`. Versions it already published
stay in the store.
