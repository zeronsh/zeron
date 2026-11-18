# Publish a Zeron plugin

Publishes a Zeron plugin to the store from a GitHub Actions job, signed
with a deploy key. The version is live at once (anyone can install it) and
marked verified once Zeron has read it. It installs Zeron's Linux command line
(checked against the release's sha256) and runs
`zeron plugin publish <folder>`. No Zeron runs and nobody signs in.

## Set up

1. Publish the plugin's first version from your own machine, with Zeron
   running and signed in (`zeron plugin publish <plugin-id>`, or Settings →
   Plugins → Yours → Publish…). That makes the id yours in the store. A deploy
   key publishes only plugins you already have there; it never claims a new
   id.
2. Make a deploy key, on your own machine with Zeron running: Settings →
   Plugins → Deploy keys, or

   ```sh
   zeron plugin key create --name "GitHub Actions" --plugins <plugin-id>
   ```

   A key is for the plugins you name (`--plugins a,b`) or for all of yours
   (`--all`, those you first publish later included), fixed when it's made.
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

A deploy key can publish new versions of its plugins and read their state,
nothing else: yanking and managing keys are yours, from your machine. A
version it publishes is live for everyone at once, so treat the secret like
a password: if it leaks, whoever has it can publish versions of those
plugins that people can install (marked not verified until Zeron reads
them; devices never take one in an "update all" and always ask before
installing one). Each version a key publishes shows as a notice on your
devices. Revoke a leaked key at once (`zeron plugin key revoke <key>`), and
yank what it sent (`zeron plugin yank <plugin-id> <version>`).

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
| `dry-run` | `false` | `true`: check, sign and pack the plugin, but don't publish it. Only `true` or `false`: anything else (`yes`, `True`, `1`) fails the step rather than submit. |
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
| 1 | The plugin doesn't meet the store's rules (a missing README, a version that isn't strict semver or was published already, …). Fix it and push again. |
| 2 | Set-up: no plugin in that folder, the secret is damaged, or the runner's clock is more than five minutes out. |
| 3 | The store refused the key or the account: the key was revoked, has expired or is another plugin's, the account has no username yet, or the id is someone else's. |
| 4 | The store couldn't be reached, or what it received was damaged on the way. Re-run the job. |

Revoke a key you no longer use, or one that may have leaked, with
`zeron plugin key revoke <plugin-id> <key>`. Versions it already published
stay in the store.
