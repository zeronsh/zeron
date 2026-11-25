# Publish a Zeron plugin or template

Publishes a Zeron plugin, or a template, to the store from a GitHub Actions
job, with a deploy token. A folder with `zeron-plugin.json` is a plugin's, one
with `zeron-template.json` a template's (both, or neither, fails the step);
what's said of plugins here goes for templates. The version is live at once (anyone can install it) and
marked verified once Zeron has read it. It installs Zeron's Linux command line
(checked against the release's sha256) and runs
`zeron plugin publish <folder>` (a template's: `zeron template publish`). No
Zeron runs and nobody signs in.

## Set up

1. Publish the plugin's first version from your own machine, with Zeron
   running and signed in (`zeron plugin publish <plugin-id>`, or Settings →
   Plugins → Yours → Publish…; a template's, `zeron template publish
   <template-id>`). That makes the id yours in the store. A deploy
   token publishes only plugins you already have there; it never claims a new
   id.
2. Make a deploy token: in Zeron (Settings → Plugins → Deploy tokens), on the
   store's site, or on your own machine with Zeron running:

   ```sh
   zeron plugin token create --name "GitHub Actions" --plugins <plugin-id>
   ```

   A token is for the plugins and templates you name (`--plugins a,b`) or for
   all of your plugins (`--all`, those you first publish later included; your
   templates too with `--all --templates`), fixed when it's made.
   The token (`zdt_…`) is shown once: neither Zeron nor the store keeps it.
3. Add it to the repository: Settings → Secrets and variables → Actions →
   New repository secret, named `ZERON_DEPLOY_TOKEN`.
4. Add the step to a workflow, pinned to a Zeron release tag (or a commit
   SHA), never a branch, so the step that holds your token doesn't change
   under you:

   ```yaml
   - uses: zeronsh/zeron/actions/publish-plugin@v0.4.0
     with:
       token: ${{ secrets.ZERON_DEPLOY_TOKEN }}
   ```

A deploy token can publish new versions of its plugins, nothing else:
reading their state, yanking and managing tokens are yours, from your machine. A
version it publishes is live for everyone at once, so treat the token like a
password: if it leaks, whoever has it can publish versions of those plugins
that people can install (marked not verified until Zeron reads them; devices
never take one in an "update all" and always ask before installing one).
Each version a token publishes shows as a notice on your devices, with the
token's name. Revoke a leaked token at once (`zeron plugin token revoke <id>`),
and yank what it sent (`zeron plugin yank <plugin-id> <version>`, or
`zeron template yank`).

What a token publishes isn't signed by your key: the store records the
version as the token's, and devices install it as yours, by the store's
record of its exact contents.

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
          token: ${{ secrets.ZERON_DEPLOY_TOKEN }}
```

Bump `version` in `zeron-plugin.json` (and add its section to
`CHANGELOG.md`) before you tag: the store takes each version once.

## Inputs

| input | default | |
|---|---|---|
| `token` | none | The deploy token. Without it the plugin is only checked, so a pull request from a fork, which gets no secrets, still runs the store's checks. |
| `path` | `.` | The plugin's or template's folder: the one with `zeron-plugin.json` or `zeron-template.json`. |
| `dry-run` | `false` | `true`: check and pack the plugin, but don't publish it. Only `true` or `false`: anything else (`yes`, `True`, `1`) fails the step rather than submit. |
| `zeron-version` | `latest` | The Zeron release whose command line runs. `latest` follows each new release; a version (`0.4.0`) runs that release, and the step fails if its manifest names another. |
| `sha256` | none | The sha256 of that release's command line tarball for the runner (`zeron-<version>-linux-x86_64.tar.gz` or `-aarch64`, from the release's `manifest-<version>.json`). With a pinned `zeron-version`, the step runs exactly that command line or fails. |

The action's tag pins the script; the command line it runs is fetched from
Zeron's releases, checked against the release's sha256. With
`zeron-version: latest` that is whichever release is newest when the job
runs. To hold the step still, the command line included, pin both:

```yaml
- uses: zeronsh/zeron/actions/publish-plugin@v0.4.0
  with:
    token: ${{ secrets.ZERON_DEPLOY_TOKEN }}
    zeron-version: 0.4.0
    sha256: <the tarball's sha256 for your runner>
```

## When it fails

The job's log says why, one line per problem, and the step exits with:

| code | meaning |
|---|---|
| 1 | The plugin doesn't meet the store's rules (a missing README, a version that isn't strict semver or was published already, …). Fix it and push again. |
| 2 | Set-up: no plugin or template in that folder (or both), or the secret isn't a deploy token or is mistyped (its last characters are a checksum). Nothing was sent. |
| 3 | The store refused the token or the account: the token was revoked, has expired or is for other plugins, the account has no username yet, or the id is someone else's. |
| 4 | The store couldn't be reached. Re-run the job. |

Revoke a token you no longer use, or one that may have leaked, with
`zeron plugin token revoke <id>` (`zeron plugin token list` shows each
token's id). Versions it already published stay in the store.
