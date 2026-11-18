# Publish a Zeron plugin

Submits a Zeron plugin to the store for review from a GitHub Actions job,
signed with the plugin's deploy key. It installs Zeron's Linux command line
(checked against the release's sha256) and runs
`zeron plugin publish <folder>`. No Zeron runs and nobody signs in.

## Set up

1. Make a deploy key for the plugin, on your own machine with Zeron
   running: Settings → Plugins → Yours → the plugin → Deploy keys, or

   ```sh
   zeron plugin key create <plugin-id> --name "GitHub Actions"
   ```

   Its secret (`zpk_…`) is shown once. Zeron keeps no copy.
2. Add the secret to the repository: Settings → Secrets and variables →
   Actions → New repository secret, named `ZERON_DEPLOY_KEY`.
3. Add the step to a workflow:

   ```yaml
   - uses: zeronsh/zeron/actions/publish-plugin@main
     with:
       key: ${{ secrets.ZERON_DEPLOY_KEY }}
   ```

For example, to publish each time you push a tag:

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
      - uses: zeronsh/zeron/actions/publish-plugin@main
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
| `zeron-version` | `latest` | The Zeron release whose command line runs. |

## When it fails

The job's log says why, one line per problem, and the step exits with:

| code | meaning |
|---|---|
| 1 | The plugin doesn't meet the store's rules (a missing README, a version that isn't strict semver or was submitted already, …). Fix it and push again. |
| 2 | Set-up: no plugin in that folder, or the secret is damaged. |
| 3 | The store refused the key or the account: the key was revoked or is another plugin's, the account has no username yet, or the id is someone else's. |
| 4 | The store couldn't be reached. Re-run the job. |

Revoke a key you no longer use, or one that may have leaked, with
`zeron plugin key revoke <plugin-id> <key>`. Versions it already published
stay in the store.
