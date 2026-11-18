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

/** "Nov 9, 2026, 18:00 UTC". */
export const dateTime = (ms: number) => DATE_TIME.format(ms);

/** "3 days ago", "yesterday", "in 2 months". */
export function ago(ms: number, now = Date.now()): string {
  const seconds = (ms - now) / 1000;
  const units: [Intl.RelativeTimeFormatUnit, number][] = [["year", 31_536_000], ["month", 2_592_000], ["week", 604_800], ["day", 86_400], ["hour", 3_600], ["minute", 60]];
  for (const [unit, size] of units) if (Math.abs(seconds) >= size) return RELATIVE.format(Math.round(seconds / size), unit);
  return "just now";
}

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

/** A time the edge gives as ms since the epoch or as an ISO date, in ms (null if it's neither). */
export function toMs(value: number | string | null | undefined): number | null {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string") {
    const ms = Date.parse(value);
    return Number.isFinite(ms) ? ms : null;
  }
  return null;
}

/** An account's age, plainly: "2 days old", "1 year old" ("" when it isn't known). */
export function age(createdAt: number | string | null | undefined, now = Date.now()): string {
  const created = toMs(createdAt);
  if (created === null) return "";
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
