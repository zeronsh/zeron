# zeron-vault

The credential vault Worker (design: `docs/design/cloud-device.md`, "Credential vault"). It
holds users' provider credentials (Codex, Claude, GitHub, API keys) encrypted, refreshes them,
and hands enrolled devices short-lived **grants**: access tokens only, never refresh tokens.

## Trust boundary

- **No HTTP surface.** There are no routes, no workers.dev URL and no preview URLs, and `fetch`
  always answers 404. The only way in is the main edge Worker's service binding
  (`VAULT`, `entrypoint: "VaultApi"`), which passes an identity it has already verified
  (WorkOS user or runner JWT). The vault re-validates every field it receives.
- **Grants need a device signature.** The device key (Ed25519, enrolled per device) signs
  `zeron-vault-grant\n{userId}\n{deviceId}\n{provider}\n{ts}`. The `ts` must be within ±120 s and
  strictly greater than the device's last one. The device must be enrolled, not revoked, and
  listed in the credential's `authorizedDevices`. Runner callers may only request grants for
  their own device.
- **Cloud sessions are children.** Each Cloud session sandbox enrolls its own key with
  `parentId` = the account's logical Cloud device. A credential connected once for that parent
  covers every session. Revoking the parent (even one that never enrolled a key) refuses all its
  children, present and future. Revoking a session touches only that session.
- **Vault-side GitHub calls.** `githubRepos` lists the repositories of the App installations
  the GitHub connection can reach, using the stored user token inside the vault, and returns
  metadata only. Pagination follows `next` links on `api.github.com` only.
- **GitHub grants are installation tokens.** The user token never leaves the vault. A `github`
  grant names `repo` (`owner/name`); the vault finds the installation on `owner` among the
  user's own installations (`/user/installations`, so nobody can name someone else's) and mints
  a one-hour installation token with an App JWT. Minted tokens are cached sealed per
  installation and reused while more than 15 minutes remain.
- **Nothing decrypts from storage alone.** Records are AES-256-GCM under a KEK held as a
  Worker secret (`VAULT_KEK`), never in Durable Object storage, and imported as a
  non-extractable key. AAD `{userId}|{provider}|{generation}` binds each ciphertext to its slot.
  The trade-off of not running an external KMS: secrets and DO storage are separate stores in
  the same Cloudflare account, so a storage dump alone decrypts nothing, but a compromise of
  the Cloudflare account itself could reach both.
- **One refresher per account.** Each `(user, provider)` is its own Durable Object, with a
  single-flight refresh gate. A new generation is persisted before any grant built from it is
  returned. Zero runtime dependencies (WebCrypto only).

## Secrets

```sh
cd edge/vault
openssl rand -base64 32 | npx wrangler secret put VAULT_KEK   # the KEK, see below
npx wrangler secret put GITHUB_APP_CLIENT_ID      # GitHub App (device flow + refresh; App JWT iss)
npx wrangler secret put GITHUB_APP_CLIENT_SECRET
npx wrangler secret put GITHUB_APP_PRIVATE_KEY < zeron.private-key.pem   # installation tokens
npx wrangler secret put GITHUB_APP_SLUG           # e.g. zeron, for the install link
npx wrangler secret put CANARY_USER_ID            # optional, see Canary
```

### The GitHub App

Settings → Developer settings → GitHub Apps → New:

- **Callback URL**: any (unused); **Enable Device Flow**: on; **Expire user authorization
  tokens**: on; webhooks: off.
- **Repository permissions**: Contents (read & write), Pull requests (read & write),
  Workflows (read & write, so agents can push changes under `.github/workflows`); Metadata
  (read) is implied. No account permissions.
- **Where can this App be installed**: any account.
- Generate a private key (GitHub downloads PKCS#1 PEM; the vault accepts it as is).

There are no vars. A missing or malformed `VAULT_KEK` (anything but base64 of 32 bytes) makes
every call that touches ciphertext answer `unavailable` (503). Nothing is cached as a failure,
so fixing the secret recovers immediately.

**Keep an offline copy of `VAULT_KEK`** (e.g. in the team password manager). Cloudflare secrets
are write-only, and without the key every stored credential is lost. To keep a copy, generate
the key into a file first (`openssl rand -base64 32 > kek.txt`), store it, pipe the file into
`wrangler secret put VAULT_KEK`, then delete the file.

### Rotating the KEK

Every envelope records the id of the key that sealed it: `v2.{kid}.{iv}.{ct}`, where `kid` is
the first 16 hex chars of SHA-256 of the key. Seals always use `VAULT_KEK`. Opens accept
`VAULT_KEK` or `VAULT_KEK_PREVIOUS`.

1. Set both secrets in one deploy, so no request sees the new key without the old one:

   ```sh
   # secrets.json: {"VAULT_KEK": "<new key>", "VAULT_KEK_PREVIOUS": "<current key>"}
   npx wrangler secret bulk secrets.json && rm secrets.json
   ```

   (Doing it one secret at a time, set `VAULT_KEK_PREVIOUS` first, then `VAULT_KEK`.)
2. Records migrate lazily. Whenever a record sealed under the previous key is opened (a grant,
   a refresh, the canary), it is re-sealed under the new key, keeping the same generation and
   AAD, and persisted before the call returns. Each migration logs a `vault.rewrap` line
   (`from`/`to` kids).
3. There is no bulk re-wrap: only the canary user is enumerable. A record nobody has used since
   the rotation stays sealed under the previous key and needs `VAULT_KEK_PREVIOUS` until it is
   accessed. Removing `VAULT_KEK_PREVIOUS` makes those records unreadable. Their grants then
   answer `unavailable` with "record sealed under key {kid}, which is no longer configured", and
   in practice those users must reconnect (a fresh upload or device flow overwrites the record).
   So keep the previous key configured until the next rotation, not just for a few days.

## Canary

A daily cron (`0 6 * * *`) force-refreshes `CANARY_USER_ID`'s codex, claude and github
credentials through the same gate real grants use, and checks the GitHub App's credentials
(`GET /app` with an App JWT; status `app_valid` / `app_rejected`). For a Claude setup token, which cannot be
refreshed, it checks that more than 30 days remain. Each result is stored in that account DO's
`canary` table and logged as `{"event":"vault.canary","provider","ok","status"}`.

To set it up, create a dedicated user, connect its credentials through the normal app flows,
then set `CANARY_USER_ID` to that user's id. Alert on `vault.canary` lines with `ok=false` in
Workers observability. Statuses: `refreshed` / `valid` (ok); `missing`, `needs_reconnect`,
`upstream`, `expiring`, `not_refreshable`, `error`.

## Kill switches

| Scope | How | Effect |
| --- | --- | --- |
| Device | `revokeDevice` (Settings → Cloud, or deleting the Cloud device) | That key can never draw grants again; re-enrolling needs a new key |
| User | `setDisabled(true)` | Grants, reads and uploads refused (403); revoke/disconnect/re-enable still work |
| Global | `npx wrangler secret put VAULT_DISABLED` with value `1` | Every call refused (503) |
| Global, crypto | Delete `VAULT_KEK` (keep the offline copy) | Nothing can be decrypted (503) until it is restored |

Consumers cache grants until `expiresAt`, so a kill switch stops new grants immediately, and
already-issued tokens age out on their own. API keys get 1 h grants for this reason, and GitHub
grants are one-hour installation tokens.

## Development

```sh
npm install
npm run typecheck
npm test            # unit (Node) + workerd (vitest-pool-workers, fixed test KEKs, stubbed upstreams)
npm run deploy      # its own deploy job; never part of the edge deploy
```
