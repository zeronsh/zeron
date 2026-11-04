/**
 * What a person or an org may be called. WorkOS puts both into the emails it
 * sends for us: an invitation reads "<person> invited you to join <org>". So
 * a name must not carry a link, an address, or text that passes for us or a
 * vendor.
 *
 * - Every name: no control, format (zero-width, bidi override) or private-use
 *   characters, no line or paragraph separators, and nothing that reads as a
 *   URL, a domain or an address (`://`, `www.`, `@`, `/`, `\`, `<`, `>`, or a
 *   dot, fullwidth or ideographic dot before two or more letters).
 * - Org names also: letters from one alphabet (Latin with Cyrillic or Greek
 *   is how lookalikes are spelled; CJK beside Latin is fine), and none of the
 *   brand or staff words (`ORG_BRANDS`), read through lookalike letters and
 *   digits.
 *
 * Checked when a name is set (create, rename, `PATCH /auth/me`). Names from
 * before these rules are grandfathered: they aren't rewritten, and an org
 * keeps inviting under its name. WorkOS's template, not ours, renders the
 * email, so the edge can't neutralise characters in an old name there.
 */
import { skeleton } from "./username";

/** Why a name was refused. */
export type NameProblem = "characters" | "link" | "mixed_scripts" | "impersonation";

const BAD_CHARACTERS = /[\p{Cc}\p{Cf}\p{Co}\p{Cs}\p{Cn}\p{Zl}\p{Zp}]/u;
const LINK = /:\/\/|www\.|[@/\\<>]|[\p{L}\p{N}-][.。｡．][\p{L}]{2,}/iu;

/** Alphabets that hold lookalikes of each other's letters. */
const LOOKALIKE_SCRIPTS = ["Latin", "Cyrillic", "Greek", "Armenian", "Cherokee", "Coptic", "Georgian"] as const;
const SCRIPT_RES = LOOKALIKE_SCRIPTS.map((s) => new RegExp(`\\p{Script=${s}}`, "u"));

/** Cyrillic and Greek letters that look like Latin ones. */
const CONFUSABLE: Record<string, string> = {
  а: "a", в: "b", е: "e", ё: "e", к: "k", м: "m", н: "h", о: "o", р: "p", с: "c", т: "t", у: "y", х: "x", ѕ: "s",
  і: "i", ї: "i", ј: "j", ԁ: "d", ԛ: "q", ԝ: "w", һ: "h", ɡ: "g", ո: "n", ս: "u", α: "a", β: "b", ε: "e", η: "n",
  ι: "i", κ: "k", ν: "v", ο: "o", ρ: "p", τ: "t", υ: "u", χ: "x", ω: "w", ζ: "z", "ı": "i"
};

/** Words an org may not be named with, wherever they appear. */
const ORG_BRANDS = ["zeron", "anthropic", "openai", "github", "microsoft", "workos", "google", "apple"];
/** Words that only count as a whole word (`Apple`, not `Pineapple`). */
const WHOLE_WORD = new Set(["google", "apple"]);

const latin = (name: string): string =>
  [...name.normalize("NFKD").replace(/\p{M}/gu, "").toLowerCase()].map((c) => CONFUSABLE[c] ?? c).join("");

/** Why a person's name won't do, if it won't. Ordinary whitespace (a tab
 * between words) is a space, as it always was. */
export const nameProblem = (name: string): NameProblem | undefined => {
  const n = name.normalize("NFKC");
  if (BAD_CHARACTERS.test(n.replace(/[\t ]/g, " "))) return "characters";
  if (LINK.test(n)) return "link";
  return undefined;
};

/** `i` and `l` (and `1`, `|`, folded to `l` by `skeleton`) read alike. */
const il = (s: string): string => s.replace(/i/g, "l");

/** Why an org's name won't do, if it won't. */
export const orgNameProblem = (name: string): NameProblem | undefined => {
  const n = name.normalize("NFKC");
  if (BAD_CHARACTERS.test(n)) return "characters";
  const first = nameProblem(name);
  if (first) return first;
  if (SCRIPT_RES.filter((re) => re.test(n)).length > 1) return "mixed_scripts";
  const words = latin(n)
    .split(/[^\p{L}\p{N}]+/u)
    .filter(Boolean)
    .map((w) => il(skeleton(w)));
  const joined = il(skeleton(latin(n).replace(/[^\p{L}\p{N}]/gu, "")));
  for (const brand of ORG_BRANDS) {
    if (WHOLE_WORD.has(brand) ? words.includes(il(brand)) : joined.includes(il(brand))) return "impersonation";
  }
  return undefined;
};
