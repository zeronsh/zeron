# Boat provisioning

Install the official Boat CLI and inspect `boat new --help`, `boat forward --help`, `boat status`, and `boat limits`. Preserve interactive `boat login`, browser approval, MFA and provider safety gates. Do not automatically answer onboarding questions or buy a plan. `xlarge` is 16 vCPU / 32 GB / 251 GB disk at $0.20/hour and requires the $100/month plan or higher; obtain point-of-purchase approval if upgrading is necessary. Monthly plan credit expires; organization billing has seat and access-policy implications.

List stopped and running machines with `boat --no-update --json list --all`; resolve owner, purpose/name, current native ID and exact type before reusing one. Lost create responses must be investigated before another create. A setup request authorizes one personal box, not automatic fleet failover.

Use an owner-private personal wallet unless a reviewed organization policy is requested. In organizations, `--no-env` boxes can be usable by all members; do not store personal Crew/gcloud/Infisical credentials there without proving isolation. Disable automatic GitHub, secret and model-provider credential passing in a dedicated environment; retain only the sandbox-scoped Boat lifecycle credential needed for `sandbox.update`. Create the environment through supported provider controls with required human approval. Do not bake broad credentials into a template.

```bash
boat --no-update --json new --type xlarge --ttl 3600 --personal --environment "$BOAT_ENVIRONMENT"
```

The CLI returns JSONL; store the `ready` event's immutable `id`, then verify `boat info`. The current CLI has no create-name flag: set the owner-qualified display name with the supported dashboard or authenticated `PATCH /api/v1/sandboxes/{id}`. Never invent a `boat new --name` flag. Use that same immutable sandbox ID for all later operations.

Before long setup, confirm a sandbox-scoped credential can run `boat --no-update --json extend self --ttl 3600` inside the host. If denied, stop and obtain a key scoped to this sandbox with `sandbox.update`; never copy the controller's account-wide token. Shared setup and independent work use `keepalive.py`; a permanent `--no-auto-stop` is not the idle-sleep implementation.

The machine is Ubuntu x86_64 with Docker and systemd. Authenticate GitHub on the host and clone into `/home/user/ashler-platform`; retain that actual project path. Use `boat scp` for verified artifacts/scripts. `COMET_DEVBOX_ID` must equal the provider's current `BOAT_ID` before configuration or engine startup. Both `boat exec ID COMMAND` and `boat ssh ID COMMAND` are supported; setup with interactive authentication must retain its terminal. Do not assume the command API's default 30-second lifetime can host a long bootstrap.

Each setup mapping has its own supervised foreground forward:

```bash
boat --no-update forward "$DEVBOX_ID" --remote "$CALLBACK_PORT" --local "$CALLBACK_PORT" --bind 127.0.0.1
boat --no-update forward "$DEVBOX_ID" --remote 8085 --local 8085 --bind 127.0.0.1
boat --no-update forward "$DEVBOX_ID" --remote 3000 --local "$WEB_LOCAL_PORT" --bind 127.0.0.1
boat --no-update forward "$DEVBOX_ID" --remote 10350 --local "$TILT_LOCAL_PORT" --bind 127.0.0.1
```

Do not expose app, callback, Tilt or CDP through `boat host`, even a token-gated URL. Retain the maintained Docker-reachable web binding, not loopback-only: pods call `host.docker.internal:3000`. Verify the host firewall denies public inbound app traffic and permits only the required Docker/kind bridge path and owner SSH forwards. Tilt/CDP and credential callbacks stay loopback-only. SSH host keys/IPs change on resume; use Boat's authenticated pinning, never disable strict checking. Stop setup forwards before testing ordinary Crew turns only when the selected release actually supports automatic forwarding; otherwise keep bounded private forwards for the endpoint journey.

A plain resume can reclaim the same machine for **10 minutes after `stopped`**, with its processes still present. This is warm recovery, not snapshot proof. For cold acceptance, wait beyond that window without SSH/exec/forward requests, then use native Wake. Record the saved identity and service PIDs/start times before stopping; afterward require newly started services, inspect the provider's hydration/endpoint metadata, and check that a disposable `/tmp` marker did not survive. Never force-stop, resize, re-pin an environment or create a replacement merely to make the test cold.

`boat info` reports restore progress under `hydration`; the machine answers long before it finishes, and the last large files take the longest. Machine types stop at `xlarge` and its 251.5 GiB disk, so follow the skill's Disk budget.

Cold resume keeps the sandbox ID but starts a machine from snapshotted files and enabled systemd units, not running processes or memory; do not infer database or kind-cluster recovery from that. Docker build cache and `~/.cache` are not captured; the shared Playwright location under `~/.local/share` is selected for snapshot recovery. Ordinary processes receive no stop signal, so cleanup must not depend on shutdown hooks. Enable the selected channel's guarded Crew devbox unit and verify current native identity after every resume. Recheck Crew, Chromium, app, Tilt and pod callbacks separately. See [Boat's stop/warm-retention contract](https://docs.boat.dev/snapshots/stop.md#wait-for-the-stop-in-a-script).

Only pre-authentication images are templates. Forking a configured disk copies Crew device IDs, sessions and manually installed credentials. A new box needs fresh identity and sign-in; resume retains the existing identity. Never use a copied authenticated image as a shortcut.

Wake inspects provider state, resumes only stopped/archived boxes and starts/checks the existing host helper. A snapshot still stopping/archiving must finish before retrying wake. Unknown states, missing snapshots, auth/plan errors and mismatched IDs fail closed; no forced stop, recovery power-cycle or replacement create.

Sources: https://docs.boat.dev/machines, https://docs.boat.dev/long-running-tasks, https://docs.boat.dev/snapshots, https://docs.boat.dev/ssh-access, https://docs.boat.dev/organizations, https://docs.boat.dev/api-keys/scopes.
