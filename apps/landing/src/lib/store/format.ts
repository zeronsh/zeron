// Numbers and times as the store says them.

const COMPACT = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 });
const WHOLE = new Intl.NumberFormat("en");
const DATE = new Intl.DateTimeFormat("en", { year: "numeric", month: "short", day: "numeric", timeZone: "UTC" });
const DATE_TIME = new Intl.DateTimeFormat("en", { year: "numeric", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", hourCycle: "h23", timeZone: "UTC", timeZoneName: "short" });
const RELATIVE = new Intl.RelativeTimeFormat("en", { numeric: "auto" });

/** "12.8k installs", "1 install", "No installs yet". */
export function installs(n: number): string {
  if (n <= 0) return "No installs yet";
  return `${n < 1000 ? WHOLE.format(n) : COMPACT.format(n).toLowerCase()} ${n === 1 ? "install" : "installs"}`;
}

/** "12,840": a count in full. */
export const count = (n: number) => WHOLE.format(n);

/** "Nov 9, 2026". */
export const date = (ms: number) => DATE.format(ms);

const MONTH_DAY = new Intl.DateTimeFormat("en", { month: "short", day: "numeric", timeZone: "UTC" });
/** A day as the app says it in a list (ds/locale `month_day`, `date`): "Oct 29" this year, "Oct 29, 2025" another. */
export const day = (ms: number, now = Date.now()) => (new Date(ms).getUTCFullYear() === new Date(now).getUTCFullYear() ? MONTH_DAY.format(ms) : DATE.format(ms));

/** "Nov 9, 2026, 18:00 UTC". */
export const dateTime = (ms: number) => DATE_TIME.format(ms);

/** "3 days ago", "yesterday", "in 2 months". */
export function ago(ms: number, now = Date.now()): string {
  const seconds = (ms - now) / 1000;
  const units: [Intl.RelativeTimeFormatUnit, number][] = [["year", 31_536_000], ["month", 2_592_000], ["week", 604_800], ["day", 86_400], ["hour", 3_600], ["minute", 60]];
  for (const [unit, size] of units) if (Math.abs(seconds) >= size) return RELATIVE.format(Math.round(seconds / size), unit);
  return "just now";
}

/**
 * When, said as a list says it: "Today", "Yesterday", "3 days ago", then the
 * day ("Nov 14"). With `hours`, today is said in hours too ("2 hours ago",
 * "Just now"): for what was just published.
 */
export function since(ms: number, now = Date.now(), hours = false): string {
  const elapsed = Math.max(0, now - ms);
  if (hours && elapsed < 86_400_000) {
    const h = Math.floor(elapsed / 3_600_000);
    const m = Math.floor(elapsed / 60_000);
    return h >= 1 ? `${h} ${h === 1 ? "hour" : "hours"} ago` : m >= 1 ? `${m} ${m === 1 ? "minute" : "minutes"} ago` : "Just now";
  }
  const startOf = (t: number) => Math.floor(t / 86_400_000);
  const days = startOf(now) - startOf(ms);
  if (days <= 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days < 7) return `${days} days ago`;
  return day(ms, now);
}

/** How long since, said as a length ("5 hours", "3 days"): how long something has waited. */
export function waitedFor(ms: number, now = Date.now()): string {
  const seconds = Math.max(0, (now - ms) / 1000);
  const units: [string, number][] = [["week", 604_800], ["day", 86_400], ["hour", 3_600], ["minute", 60]];
  for (const [unit, size] of units) {
    const n = Math.floor(seconds / size);
    if (n >= 1) return `${n} ${unit}${n === 1 ? "" : "s"}`;
  }
  return "Under a minute";
}

/**
 * A deploy key's public half, short, the one way it is shown everywhere:
 * its first 6 and last 4 characters ("k5Yh3x…g2Wq"), in mono, the whole key
 * in its title. Enough to tell keys apart at a glance.
 */
export const shortKey = (key: string) => (key.length > 12 ? `${key.slice(0, 6)}…${key.slice(-4)}` : key);

/** "1.2 MB". */
export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB"];
  let value = n / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

/** An account's age, plainly: "2 days old", "1 year old" ("" for an account since deleted). */
export function age(created: number | null | undefined, now = Date.now()): string {
  if (typeof created !== "number" || !Number.isFinite(created)) return "";
  const days = Math.max(0, Math.floor((now - created) / 86_400_000));
  if (days < 1) return "New today";
  if (days < 60) return `${days} ${days === 1 ? "day" : "days"} old`;
  if (days < 730) return `${Math.floor(days / 30)} months old`;
  return `${Math.floor(days / 365)} years old`;
}

/** What a Zeron version range asks, in words: ">=0.4.0" → "Zeron 0.4.0 or later". */
export function compatibility(range: string | undefined | null): string {
  if (!range || range.trim() === "*" || range.trim() === "") return "Any version of Zeron";
  const r = range.trim();
  let m = /^>=\s*v?(\d+\.\d+\.\d+(?:-[\w.]+)?)$/.exec(r);
  if (m) return `Zeron ${m[1]} or later`;
  m = /^\^\s*v?(\d+)\.(\d+)\.(\d+)$/.exec(r);
  if (m) return `Zeron ${m[1]}.${m[2]}.${m[3]} or later, before ${m[1] === "0" ? `0.${Number(m[2]) + 1}.0` : `${Number(m[1]) + 1}.0.0`}`;
  m = /^>=\s*v?(\d+\.\d+\.\d+)\s*<\s*v?(\d+\.\d+\.\d+)$/.exec(r);
  if (m) return `Zeron ${m[1]} or later, before ${m[2]}`;
  return `Zeron ${r}`;
}

/**
 * Words an author wrote, for a place that can't hold an element of their
 * own (a page's title, a dialog's, a label read aloud): without the
 * controls that reorder text (U+061C, U+200E/F, U+202A–202E, U+2066–2069),
 * so they can't turn the words around them. Everywhere else an author's
 * words sit in an element that isolates them (app.css).
 */
export const unbidi = (text: string) => text.replace(/[\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]/g, "");
