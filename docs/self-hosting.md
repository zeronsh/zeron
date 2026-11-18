# Self-hosting a Zeron server

Zeron works on one device with no server at all. Sync (following and driving
agents from another computer or your phone) goes through a **server**: the
edge Worker in [`edge/`](../edge). By default the apps use Zeron's at
`edge.zeron.sh`. This guide runs your own on your own Cloudflare account, so
your synced chats, files and attachments stay in infrastructure you control.

Once it's deployed, share its address. Opened in a browser, it's your
server's invite page: *Join Acme on Zeron*, with a button that opens the
apps on it. The apps look up everything else themselves.

```
  Zeron app ──── GET /.well-known/zeron ────▶  your edge (Cloudflare Worker)
     │      ◀── { workosClientId, name } ────      │ Durable Objects: rooms
     │                                             │ R2: attachments, backups
     └── sign-in ──▶ WorkOS (your environment) ◀───┘ verifies sign-ins
```

## What you need

- A **Cloudflare** account with Workers, Durable Objects (SQLite) and R2.
  Check Cloudflare's current plan limits for the size of your team.
- A **WorkOS** account. AuthKit handles sign-in, accounts and
  organizations.
- Node.js 22 or later (Wrangler's minimum), and this repository.

### Why your own WorkOS

Everything except sign-in runs on your Cloudflare account. Sign-in needs a
WorkOS environment of your own because:

- Your edge exchanges sign-in codes and refreshes sessions with a WorkOS
  **API key**. That key is a secret, and Zeron can't share its own.
- A WorkOS environment only sends people back to the **redirect URIs**
  registered in it. Your server's sign-in page has to be one of them.
- Accounts live in the environment. Your server's users are yours, and
  they are separate from Zeron accounts. The apps say so: *"Your account on
  a self-hosted server is separate from your Zeron account."*

The WorkOS **client id** is public: it appears in every sign-in URL. Your
edge publishes it at `/.well-known/zeron`, so nobody has to type it.

## 1. Set up WorkOS

1. In the [WorkOS dashboard](https://dashboard.workos.com), choose the
   environment to use (Production for real use) and turn on **AuthKit**.
   Pick the sign-in methods you want (email, Google, SSO, …).
2. Under **Redirects**, add all three:

   | Redirect URI                               | Used by                                   |
   | ------------------------------------------ | ----------------------------------------- |
   | `zeron://callback`                         | the iOS and Android apps                  |
   | `http://127.0.0.1:27641/callback`          | the desktop app                           |
   | `https://<your edge>/auth/cli/callback`    | `zeron login` on a headless machine       |

   `<your edge>` is the host you deploy to in step 2, for example
   `zeron.example.com` or `zeron-edge.<you>.workers.dev`.
3. Under **API Keys**, copy the **client id** (`client_…`) and the **API
   key** (`sk_…`).

## 2. Deploy the edge

The repository ships a configuration for a server of your own:
[`edge/wrangler.selfhost.jsonc`](../edge/wrangler.selfhost.jsonc). It has
only what sync and sign-in need.

```bash
git clone https://github.com/zeronsh/zeron.git
cd zeron/edge
npm ci
npx wrangler login

# Storage for attachments and backups. The second bucket stays empty, but
# the Worker expects it.
npx wrangler r2 bucket create zeron-blobs
npx wrangler r2 bucket create zeron-releases
```

Edit `wrangler.selfhost.jsonc`:

- `WORKOS_CLIENT_ID`: the client id from step 1.
- `SERVER_NAME`: what the apps and the invite page call your server. With
  `"Acme"`, the page says *Join Acme on Zeron* and the apps' welcome
  screen shows **Acme · zeron.example.com**. Leave it empty to use the
  address instead.
- `name`: the Worker's name. Choose it now: Durable Object storage belongs
  to the name, so renaming it later starts every room empty.
- For a domain of your own, uncomment `routes`. The domain must be on a
  zone in the same Cloudflare account. Otherwise use the `workers.dev`
  address.

Then add the API key as a secret and deploy:

```bash
npx wrangler secret put WORKOS_API_KEY --config wrangler.selfhost.jsonc
npx wrangler deploy --config wrangler.selfhost.jsonc
```

### Check it

```bash
curl https://<your edge>/.well-known/zeron
```

```json
{"zeron":1,"name":"Acme","auth":{"mode":"workos","workosClientId":"client_…"}}
```

A `503` means a setting is missing, and the response names it. Only names
are printed, never values:

```json
{"zeron":1,"error":"sign_in_not_configured","missing":["WORKOS_API_KEY"]}
```

### Optional: the user directory

To keep a list of your server's users and organizations on the edge, add a
WorkOS webhook:

1. Point it at `https://<your edge>/workos/webhook`.
2. Subscribe it to `user.*`, `organization.*` and `organization_membership.*`
   (created, updated, deleted).
3. Store its signing secret:

```bash
npx wrangler secret put WORKOS_WEBHOOK_SECRET --config wrangler.selfhost.jsonc
```

With an `ADMIN_TOKEN` secret set as well, `GET /admin/users` and `GET
/admin/organizations` return those lists (`Authorization: Bearer
<ADMIN_TOKEN>`). Sync and sign-in don't need any of this.

## 3. Connect the apps

### With the invite page

Send people your edge's address, `https://<your edge>/`. The page there
has an **Open in Zeron** button: it opens the app's server dialog with your
address filled in and checked. The app switches only when they click
*Connect to Acme*, or *Sign Out and Switch* if they're signed in somewhere
else. A link alone never changes their server.

The page also has the download links, and the address with a **Copy**
button for anyone connecting by hand. It isn't shown on `zeron.sh` hosts.

### By hand

Open the server dialog:

| App     | Signed out                                       | Signed in                                                   |
| ------- | ------------------------------------------------ | ----------------------------------------------------------- |
| Desktop | **File → Connect to Server…**                    | **File → Connect to Server…** or **Settings → Account → Server** |
| iPhone  | **⋯** at the top right → **Connect to Server…**  | **Settings → Server**                                       |
| Android | **⋮** at the top right → **Connect to server…**  | **Settings → Server**                                       |

1. Choose **Self-hosted**.
2. Type the address, for example `zeron.example.com`.
3. Click **Connect**.

The app checks the address before changing anything:

- **Signed out**: the app switches right away.
- **Signed in**: it first asks *"Switch to Acme?"*. Switching signs you out
  on that device, then setup opens so you can sign in on the new server.

While Zeron's own server is in use, the welcome screen doesn't mention
servers. On a self-hosted one, it shows the server's name and address
above the title. Click or tap it to change server.

### Headless (`zeron` CLI)

Point the engine at the server, then sign in.
`zeron login` prints a link. Open it in any browser, sign in, and paste the
code back.

```bash
zeron daemon stop
export ZERON_EDGE_URL=https://zeron.example.com
export ZERON_WORKOS_CLIENT_ID=client_…
zeron login
zeron daemon install   # captures the ZERON_* variables for the service
```

Addresses use `https://`. Plain `http://` works only for servers on your
own network: `localhost`, private and Tailscale IPs, `.local`, `.lan`,
`.internal` and `.home.arpa` names.

### Where the choice is kept

The desktop app writes the server to `server.json` in its engine's data
directory. The engine there uses it on its next start, including when
something other than the app starts that engine. The phone apps keep it
on the phone.

For the `zeron` CLI and engine, `ZERON_EDGE_URL` or
`ZERON_WORKOS_CLIENT_ID` in the environment wins over `server.json`. Unset
them to go back to the saved choice.

To go back to Zeron's server, choose **Zeron** in the same dialog.

## What a self-hosted server doesn't do

| Feature                    | Why                                                                                     |
| -------------------------- | --------------------------------------------------------------------------------------- |
| iOS push notifications     | Push for Zeron's iOS app goes through Zeron's Apple key, which your server can't use.   |
| Zeron Cloud and billing    | Cloud machines and plans are Zeron's service (`/cloud` answers that it's unavailable).  |
| Member card pages          | `zeron.sh/@name` pages come from Zeron's edge.                                          |
| Hosted model list          | Engines use the model lists built into them.                                            |
| Updates                    | Apps keep updating from Zeron's releases, whichever server they sync with.              |

Local work doesn't change. Sessions on a device stay there, and nothing
already local is uploaded when you sign in.

## Keeping it up to date

Redeploy the edge from the same release as your apps:

```bash
git pull
cd edge && npm ci
npx wrangler deploy --config wrangler.selfhost.jsonc
```

If a release adds a Durable Object migration (a new `tag` in
`edge/wrangler.jsonc`), copy that tag into `wrangler.selfhost.jsonc` too.
Never edit or remove a tag that has already been deployed.

Apps and servers say when they don't match:

- **Server too old**: *"… runs an older Zeron server. Its owner needs to
  update it."*
- **App too old**: *"… needs a newer version of Zeron. Update Zeron to
  connect."*

## When the app says…

| Message                                             | What to check                                                                                      |
| --------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| *Couldn't reach …*                                  | The address, DNS, and whether `curl https://<your edge>/health` answers.                           |
| *… isn't a Zeron server.*                           | The address points at something else, for example a proxy or another site.                         |
| *… hasn't finished setting up sign-in (missing …)*  | Set the named variable or secret, then redeploy.                                                   |
| *… is a development server …*                       | `AUTH_MODE` is `dev`. Set it to `workos`.                                                          |
| *Use https://. …*                                   | The server is outside your network. Put it behind TLS (`workers.dev` and custom domains have it).  |
| Sign-in opens, then fails                           | One of the three redirect URIs from step 1 is missing in WorkOS.                                   |
