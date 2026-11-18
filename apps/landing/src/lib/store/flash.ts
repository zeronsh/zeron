// What a form did, said once on the page it comes back to. Only a word goes
// in the cookie, and only these words are ever shown: nothing a visitor or
// the edge writes reaches the page through it.
import type { AstroCookies } from "astro";

export const FLASH = {
  withdrawn: { message: "Withdrawn from review.", tone: "success" },
  yanked: { message: "No longer offered. Anyone who has it keeps it.", tone: "success" },
  unyanked: { message: "Offered again.", tone: "success" },
  key_revoked: { message: "Key revoked. It can't publish anything now.", tone: "success" },
  approved: { message: "Approved. It's listed now.", tone: "success" },
  rejected: { message: "Not approved. The author can read why.", tone: "success" },
  removed: { message: "Taken down.", tone: "success" },
  suspended: { message: "Suspended. It's gone from the store.", tone: "success" },
  restored: { message: "Restored to the store.", tone: "success" },
  featured: { message: "Featured.", tone: "success" },
  unfeatured: { message: "No longer featured.", tone: "success" },
  transferred: { message: "Moved to its new owner.", tone: "success" },
  report_resolved: { message: "Report closed.", tone: "success" },
  reserved: { message: "Reserved.", tone: "success" },
  reservation_released: { message: "The id is free to claim.", tone: "success" },
  id_in_use: { message: "That id is in use: move its plugin to the account instead.", tone: "danger" },
  report_sent: { message: "Thanks. Your report reached the people who review plugins.", tone: "success" },
  failed: { message: "That didn't work. Try again in a moment.", tone: "danger" },
  slow_down: { message: "That's a lot of changes at once. Wait a minute, then try again.", tone: "danger" },
  conflict: { message: "It changed while you were looking. Here it is now.", tone: "danger" },
  not_allowed: { message: "You can't do that here.", tone: "danger" },
  expired: { message: "The page was open too long. Try again.", tone: "danger" },
  bad_input: { message: "Something it needs is missing. Fill it in and try again.", tone: "danger" },
} as const satisfies Record<string, { message: string; tone: "success" | "danger" }>;

export type FlashWord = keyof typeof FLASH;

const NAME = "__Host-zeron_flash";

export function setFlash(cookies: AstroCookies, word: FlashWord): void {
  cookies.set(NAME, word, { path: "/", secure: true, httpOnly: true, sameSite: "lax", maxAge: 60 });
}

/** The flash waiting for this page, taken (it shows once). */
export function takeFlash(cookies: AstroCookies): ((typeof FLASH)[FlashWord] & { word: FlashWord }) | null {
  const word = cookies.get(NAME)?.value;
  if (word === undefined) return null;
  cookies.delete(NAME, { path: "/", secure: true, httpOnly: true, sameSite: "lax" });
  return Object.hasOwn(FLASH, word) ? { ...FLASH[word as FlashWord], word: word as FlashWord } : null;
}
