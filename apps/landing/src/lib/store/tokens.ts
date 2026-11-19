// Making a deploy token on the site (contract a315eef5a): the form's fields
// read from a post or from the address it comes back to after signing in
// again, what's sent to the edge, and what each refusal says. The token
// itself never passes through here: it's in the edge's answer, drawn once
// on the page that answers the post (pages/account/tokens/new.astro), and
// nowhere else.
import type { Result } from "./edge.ts";
import { field } from "./forms.ts";
import type { TokenScope } from "./types.ts";
import { signInUrl } from "./urls.ts";
import { store } from "./words.ts";

/** A token's id, as the edge names it apart from its secret. */
export const isTokenId = (value: unknown): value is string => typeof value === "string" && /^tok_[0-9A-Za-z]{16}$/.test(value);

/** When a token ends: the form's choices, in days; "never" sends no `expires`. */
export const ENDS = ["30", "90", "365", "never"] as const;
export type Ends = (typeof ENDS)[number];
const isEnds = (value: string): value is Ends => (ENDS as readonly string[]).includes(value);

/** The most plugins a token's scope may name (the edge's MAX_DEPLOY_TOKEN_PLUGINS). */
export const MOST_PLUGINS = 50;
const PLUGIN_ID = /^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$/;

/** What the form holds: as typed, or as it came back. */
export interface TokenFields {
  name: string;
  ends: Ends;
  scope: "all" | "chosen";
  plugins: string[];
}

export const NO_FIELDS: TokenFields = { name: "", ends: "90", scope: "all", plugins: [] };

const plugins = (values: string[]) => [...new Set(values.filter((id) => PLUGIN_ID.test(id)))].slice(0, MOST_PLUGINS);

/** The fields of a post. */
export function postedFields(form: FormData): TokenFields {
  const ends = field(form, "ends", 8);
  return {
    name: field(form, "name", 200),
    ends: isEnds(ends) ? ends : NO_FIELDS.ends,
    scope: field(form, "scope", 8) === "chosen" ? "chosen" : "all",
    plugins: plugins(form.getAll("plugins").filter((v): v is string => typeof v === "string")),
  };
}

/** The fields in the address the form comes back to (`/account/tokens/new?name=…`). */
export function addressFields(url: URL): TokenFields {
  const ends = url.searchParams.get("ends") ?? "";
  return {
    name: (url.searchParams.get("name") ?? "").slice(0, 200),
    ends: isEnds(ends) ? ends : NO_FIELDS.ends,
    scope: url.searchParams.get("scope") === "chosen" ? "chosen" : "all",
    plugins: plugins((url.searchParams.get("plugins") ?? "").split(",")),
  };
}

/**
 * Sign in again, then back to the form as it was filled in. What travels is
 * the name and the choices, never the token: there isn't one yet. (A name
 * is 64 characters at most and fifty plugin ids fit in the 2,048 the edge
 * brings back.)
 */
export function signInAgain(fields: TokenFields): string {
  const params = new URLSearchParams({ again: "1" });
  if (fields.name) params.set("name", fields.name.slice(0, 64));
  params.set("ends", fields.ends);
  params.set("scope", fields.scope);
  if (fields.scope === "chosen" && fields.plugins.length) params.set("plugins", fields.plugins.join(","));
  return signInUrl(`/account/tokens/new?${params}`);
}

/** What the edge is asked to make, or what's wrong with the form first. */
export function tokenRequest(fields: TokenFields, now: number): { name: string; scope: TokenScope; expires?: number } | Refusal {
  if (!fields.name || fields.name.length > 64) return { field: "name", message: "Name the token in 1 to 64 characters." };
  if (fields.scope === "chosen" && fields.plugins.length === 0) return { field: "plugins", message: "Choose at least one plugin it can publish." };
  const scope: TokenScope = fields.scope === "all" ? { all: true } : { plugins: fields.plugins };
  return fields.ends === "never" ? { name: fields.name, scope } : { name: fields.name, scope, expires: now + Number(fields.ends) * 86_400_000 };
}

/** A refusal, said beside the field it's about (none: above the buttons). */
export interface Refusal {
  field: "name" | "plugins" | null;
  message: string;
}
export const isRefusal = (value: unknown): value is Refusal => typeof value === "object" && value !== null && "message" in value && "field" in value;

/** What the edge's refusal says on the form; `step_up` when it wants a fresh sign-in instead. */
export function refusalOf(result: Extract<Result<unknown>, { ok: false }>, name: (id: string) => string): Refusal | "step_up" | "signed_out" {
  const names = (ids: string[] | undefined) => (ids?.length ? ids.map(name).join(", ") : "");
  switch (result.error) {
    case "step_up_required":
      return "step_up";
    case "signed_out":
    case "unauthenticated":
      return "signed_out";
    case "invalid_name":
      return { field: "name", message: "Name the token in 1 to 64 characters, with no line breaks." };
    case "invalid_scope":
      return { field: "plugins", message: result.ids?.length ? `These aren't plugins it can publish: ${names(result.ids)}.` : `Choose 1 to ${MOST_PLUGINS} plugins it can publish.` };
    case "not_owner":
      return { field: "plugins", message: result.ids?.length ? `You don't own ${names(result.ids)} any more. Choose again.` : "You don't own one of those plugins any more. Choose again." };
    case "too_many_tokens":
      return { field: null, message: "You have 20 deploy tokens, the most an account can have. Revoke one to make another." };
    case "rate_limited":
      return { field: null, message: store("plugins.store.key.made_today") };
    case "invalid_expiry":
      return { field: null, message: "That end date didn't work. Choose when it ends again." };
    default:
      return { field: null, message: "The plugin store didn't make the token. Try again in a moment." };
  }
}
