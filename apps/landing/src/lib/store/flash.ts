// What a form did, said once on the page it comes back to. Only a word goes
// in the cookie, with the versions its message names, if any, and only these
// words are ever shown: nothing a visitor or the edge writes reaches the page
// through it but a version, held to what a version may be.
import type { AstroCookies } from "astro";
import { isVersion } from "./names.ts";

export const FLASH = {
  yanked: { message: "New installs stopped. Those who have it keep it working.", tone: "success" },
  unyanked: { message: "It can be installed again.", tone: "success" },
  key_revoked: { message: "Token revoked. It can't publish anything now.", tone: "success" },
  verified: { message: "Verified. Anyone installing it sees so.", tone: "success" },
  declined: { message: "Not verified. It stays listed; the author can read why.", tone: "success" },
  taken_down: { message: "Taken down, and turned off on the devices that have it.", tone: "success" },
  removed: { message: "Removed. Those who have it keep it.", tone: "success" },
  version_restored: { message: "Restored: listed again, not verified.", tone: "success" },
  // (Newer, not "listed": the edge names the newest of those listed or stopped.)
  version_replaced: { message: (version, by) => `Restored, not verified. ${by} is newer, so ${version} takes no new installs.`, tone: "success" },
  version_back: { message: "Restored, not verified.", tone: "success" },
  not_verified: { message: "Only a plugin with a verified version can be featured.", tone: "danger" },
  suspended: { message: "Suspended. It's gone from the store.", tone: "success" },
  restored: { message: "Restored to the store.", tone: "success" },
  featured: { message: "Featured.", tone: "success" },
  unfeatured: { message: "No longer featured.", tone: "success" },
  transferred: { message: "Moved to its new owner.", tone: "success" },
  report_resolved: { message: "Report closed.", tone: "success" },
  reserved: { message: "Reserved.", tone: "success" },
  reservation_released: { message: "The id is free to claim.", tone: "success" },
  id_in_use: { message: "That id is in use: move its plugin to the account instead.", tone: "danger" },
  site_id: { message: "That id is one of the site's pages: no plugin can have it.", tone: "danger" },
  report_sent: { message: "Thanks. Your report reached the people who review plugins.", tone: "success" },
  failed: { message: "That didn't work. Try again in a moment.", tone: "danger" },
  slow_down: { message: "That's a lot of changes at once. Wait a minute, then try again.", tone: "danger" },
  conflict: { message: "It changed while you were looking. Here it is now.", tone: "danger" },
  not_allowed: { message: "You can't do that here.", tone: "danger" },
  expired: { message: "The page was open too long. Try again.", tone: "danger" },
  bad_input: { message: "Something it needs is missing. Fill it in and try again.", tone: "danger" },
} as const satisfies Record<string, { message: string | ((version: string, by: string) => string); tone: "success" | "danger" }>;

export type FlashWord = keyof typeof FLASH;

/** A word, and the versions its message names. */
export type Said = FlashWord | readonly [word: FlashWord, version: string, by: string];

const NAME = "__Host-zeron_flash";
// (No word has one, nor does a version.)
const BETWEEN = "~";

export function setFlash(cookies: AstroCookies, said: Said): void {
  cookies.set(NAME, typeof said === "string" ? said : said.join(BETWEEN), { path: "/", secure: true, httpOnly: true, sameSite: "lax", maxAge: 60 });
}

/** The flash waiting for this page, taken (it shows once). */
export function takeFlash(cookies: AstroCookies): { message: string; tone: "success" | "danger"; word: FlashWord } | null {
  const said = cookies.get(NAME)?.value;
  if (said === undefined) return null;
  cookies.delete(NAME, { path: "/", secure: true, httpOnly: true, sameSite: "lax" });
  const [word, version, by, ...more] = said.split(BETWEEN);
  if (word === undefined || !Object.hasOwn(FLASH, word)) return null;
  const { message, tone } = FLASH[word as FlashWord];
  if (typeof message === "string") return version === undefined ? { message, tone, word: word as FlashWord } : null;
  return isVersion(version) && isVersion(by) && more.length === 0 ? { message: message(version, by), tone, word: word as FlashWord } : null;
}
