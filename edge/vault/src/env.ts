/**
 * The vault Worker's bindings (wrangler.jsonc). Everything except the two DO
 * namespaces is a secret or var and may be absent: a missing secret makes the
 * affected operation answer `unavailable`, never crash the isolate.
 */
import type { VaultAccount } from "./account";
import type { VaultDevices } from "./devices";

export interface Env {
  readonly VAULT_DEVICES: DurableObjectNamespace<VaultDevices>;
  readonly VAULT_ACCOUNTS: DurableObjectNamespace<VaultAccount>;

  /** Global kill switch: `"1"` refuses every call with 503. */
  readonly VAULT_DISABLED?: string;

  /** The KEK: base64 of 32 random bytes. Required; seals every record. */
  readonly VAULT_KEK?: string;
  /** The KEK before the last rotation: still opens records sealed under it
   * (which are re-sealed under VAULT_KEK as they are accessed). */
  readonly VAULT_KEK_PREVIOUS?: string;

  readonly GITHUB_APP_CLIENT_ID?: string;
  readonly GITHUB_APP_CLIENT_SECRET?: string;
  /** The App's private key (PEM, PKCS#1 as GitHub downloads it, or PKCS#8):
   * signs the App JWT that mints installation tokens for Cloud sessions. */
  readonly GITHUB_APP_PRIVATE_KEY?: string;
  /** The App's URL slug (`github.com/apps/{slug}`), for the install link. */
  readonly GITHUB_APP_SLUG?: string;

  /** User whose codex + github credentials the daily cron force-refreshes. */
  readonly CANARY_USER_ID?: string;
}

/** Device ids name DO rows and sit inside signed `\n`-separated messages, so
 * they must be printable, space-free ASCII: a newline in an id would let one
 * signed message parse as another. User ids double as DO-name components. */
export const ID_RE = /^[\x21-\x7e]{1,128}$/;
