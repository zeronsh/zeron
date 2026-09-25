# Glitch Flow cloud setup (not deployed)

GitHub identifies a person; the Glitch Flow edge connects that person's devices
and stores synced data. The edge URL is an operator-controlled service address,
not a field each user should enter. A Cloudflare `workers.dev` URL is enough
to start; no custom domain is required. No Glitch Flow cloud service has been
deployed by this setup.

## Set up accounts and callbacks

1. In a Glitch Flow-owned Cloudflare account, enable its
   [`workers.dev` subdomain](https://developers.cloudflare.com/workers/configuration/routing/workers-dev/).
   The Worker in [`edge/wrangler.jsonc`](../edge/wrangler.jsonc) will use
   `https://glitch-flow-edge.<your-subdomain>.workers.dev`. Create the two
   [R2 buckets](https://developers.cloudflare.com/r2/buckets/create-buckets/)
   named `glitch-flow-blobs` and `glitch-flow-releases` in that account.
   Keep the Worker name stable after first deploy: Durable Object data belongs
   to that Worker name.
2. Create a Glitch Flow [WorkOS AuthKit application](https://workos.com/docs/authkit/applications)
   and record its public `client_...` ID and secret API key. In WorkOS
   **Authentication → OAuth providers → GitHub**, copy the GitHub redirect
   URI shown there. Create a GitHub OAuth App, use that **WorkOS URI** as its
   authorization callback, ensure WorkOS requests the required `user:email`
   scope, and enter its client ID and secret back into WorkOS.
   [WorkOS's GitHub guide](https://workos.com/docs/integrations/github-oauth)
   gives the dashboard steps. Enable GitHub in AuthKit. If GitHub should be the
   only sign-in choice, disable the default Email + Password method in the
   WorkOS Authentication settings.
3. In the WorkOS application's **Redirects** tab, allow both
   `http://127.0.0.1:*/callback` for the desktop's ephemeral loopback port
   and `https://glitch-flow-edge.<your-subdomain>.workers.dev/auth/cli/callback`
   for headless sign-in. WorkOS [supports a wildcard loopback port](https://workos.com/docs/reference/authkit/authentication/get-authorization-url).
   These are callbacks from WorkOS to Glitch Flow; the GitHub OAuth App
   callback in step 2 points to WorkOS.
4. Replace `REPLACE_WITH_WORKOS_CLIENT_ID` in `edge/wrangler.jsonc`.
   From `edge/`, set `WORKOS_API_KEY` as a
   [Wrangler secret](https://developers.cloudflare.com/workers/configuration/secrets/)
   with `npx wrangler secret put WORKOS_API_KEY`. Never put this API key or
   the GitHub OAuth secret in source control or a desktop package.

## Deploy and connect clients

The edge [deployment workflow](../.github/workflows/deploy.yml) now runs only
by manual `workflow_dispatch` and deploys only the Glitch Flow Worker. Set
this repository's `CLOUDFLARE_ACCOUNT_ID` variable to the new account and
`CLOUDFLARE_API_TOKEN` secret to a token for that account before running it.
It refuses the inherited Zeron account and an unchanged WorkOS placeholder.
Run the edge tests and verify `/health` reports `auth: "workos"` after
deployment. The release workflow now checks for a Glitch Flow Cloudflare
account and targets `glitch-flow-releases`, but it remains unvalidated.
Do not run it until account access, storage, signing, and platform packages
have been reviewed.

For a friend-ready release, set the public values
`GLITCH_FLOW_EDGE_URL=https://glitch-flow-edge.<your-subdomain>.workers.dev`
and `GLITCH_FLOW_WORKOS_CLIENT_ID=client_...` in the **build environment**
when compiling `glitch-flow`. Set the matching repository Actions variables
`GLITCH_FLOW_EDGE_URL` and `GLITCH_FLOW_WORKOS_CLIENT_ID` before a release;
the release workflow requires them and passes them to each platform build.
The executable bundles them; a runtime override can still be used for local
tests. Existing installed binaries must be rebuilt with those values before
they offer sign-in. Users then choose **Enable Sync** and GitHub on the hosted
sign-in page. Each person uses their own GitHub account across their devices.
Sharing one GitHub identity would also share access to its device rooms and
workspaces.

## Before moving existing work

The present development profile is under `orgs/dev-org/dev-user`. Signing in
creates a different, account-scoped synced profile. The existing local-profile
import does not migrate this development profile, so back up its data and
implement a deliberate development-to-synced migration before switching the
working installation. The current edge persists synced documents and blobs;
end-to-end encryption is not implemented. A production client should also
reject a development-auth edge instead of silently downgrading authentication.
