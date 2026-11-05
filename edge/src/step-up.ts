/**
 * Step-up tickets: proof that a person just signed in, for what a bearer alone
 * must not do. Today that's binding a new plugin signing key (plugin-keys.ts)
 * and deleting the account (`DELETE /auth/me`).
 *
 * A bearer is easy to come by on a device: any local process the engine hands
 * its access token to has one. A WorkOS authorization code isn't, because it
 * comes only from an interactive sign-in. So every successful code exchange
 * (`POST /auth/exchange`) mints a ticket, answered beside the tokens as
 * `stepUp: { token, expiresAt }`. `/auth/refresh` never mints one.
 *
 * - Random 32 bytes, b64url. The directory stores only its SHA-256, the user
 *   it was minted for and its expiry (`step_ups` in user-directory.ts).
 * - Valid for `STEP_UP_TTL_MS`, for the user whose sign-in made it. Single
 *   use: the action it allows uses it up.
 * - Requests carry the token string as `stepUp` in the JSON body. Without a
 *   live ticket the answer is 403 `{error: "step_up_required"}`. That's never
 *   a 409, because engines in the field rotate their key on any 409 from
 *   `POST /plugin-keys`.
 *
 * Headless devices sign in through the paste-code flow (`/auth/cli/callback`),
 * which is a code exchange too.
 */
import { b64urlEncode } from "./cloud/crypto";

/** How long a sign-in's ticket lasts. */
export const STEP_UP_TTL_MS = 10 * 60_000;
/** Live tickets kept per user. Past this, the oldest go. */
export const MAX_LIVE_STEP_UPS = 5;

const TOKEN_RE = /^[A-Za-z0-9_-]{43}$/;

export const newStepUpToken = (): string => b64urlEncode(crypto.getRandomValues(new Uint8Array(32)));

/** The stored form of a ticket: SHA-256, hex. Undefined for something that
 * isn't one. */
export const stepUpHash = async (token: unknown): Promise<string | undefined> => {
  if (typeof token !== "string" || !TOKEN_RE.test(token)) return undefined;
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(token));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
};

export const stepUpRequired = (): Response =>
  new Response(
    JSON.stringify({
      error: "step_up_required",
      message: "Sign in again to do this. A fresh sign-in is needed, not just this device's session."
    }),
    { status: 403, headers: { "content-type": "application/json" } }
  );
