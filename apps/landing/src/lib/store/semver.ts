// Whether a Zeron version is in a plugin's `engines.zeron` range, as npm's
// ranges read (what the engine checks before it installs): comparators
// (>=, >, <=, <, =, ^, ~, a bare version, "*" and x's) joined by spaces (all
// must hold) and "||" (any may). A range it can't read is no answer (null):
// the page then says nothing, rather than something wrong.

type Version = [number, number, number, string];

function parse(text: string): Version | null {
  const m = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/.exec(text.trim());
  return m ? [Number(m[1]), Number(m[2]), Number(m[3]), m[4] ?? ""] : null;
}

function compare(a: Version, b: Version): number {
  for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return (a[i] as number) - (b[i] as number);
  // A pre-release comes before its release.
  if (a[3] === b[3]) return 0;
  if (!a[3]) return 1;
  if (!b[3]) return -1;
  return a[3] < b[3] ? -1 : 1;
}

/** One comparator, as the versions it allows: a test, or null when unreadable. */
function comparator(text: string): ((v: Version) => boolean) | null {
  const c = text.trim();
  if (c === "" || c === "*" || /^[xX*](\.[xX*]){0,2}$/.test(c)) return () => true;
  const m = /^(>=|<=|>|<|=|\^|~)?\s*v?(\d+)(?:\.(\d+|[xX*]))?(?:\.(\d+|[xX*]))?(?:-([0-9A-Za-z.-]+))?$/.exec(c);
  if (!m) return null;
  const op = m[1] ?? "";
  const [major, minor, patch] = [Number(m[2]), m[3] && /\d/.test(m[3]) ? Number(m[3]) : null, m[4] && /\d/.test(m[4]) ? Number(m[4]) : null];
  const low: Version = [major, minor ?? 0, patch ?? 0, m[5] ?? ""];
  const at = (v: Version) => compare(v, low);
  // A partial version ("1.2") is the range of what it leaves open.
  const upper = (): Version => (minor === null ? [major + 1, 0, 0, ""] : patch === null ? [major, minor + 1, 0, ""] : [major, minor, patch + 1, ""]);
  switch (op) {
    case ">=":
      return (v) => at(v) >= 0;
    case ">":
      return (v) => (patch === null ? compare(v, upper()) >= 0 : at(v) > 0);
    case "<=":
      return (v) => (patch === null ? compare(v, upper()) < 0 : at(v) <= 0);
    case "<":
      return (v) => at(v) < 0;
    case "^": {
      const top: Version = major > 0 || minor === null ? [major + 1, 0, 0, ""] : (minor ?? 0) > 0 || patch === null ? [0, (minor ?? 0) + 1, 0, ""] : [0, 0, (patch ?? 0) + 1, ""];
      return (v) => at(v) >= 0 && compare(v, top) < 0;
    }
    case "~": {
      const top: Version = minor === null ? [major + 1, 0, 0, ""] : [major, minor + 1, 0, ""];
      return (v) => at(v) >= 0 && compare(v, top) < 0;
    }
    default:
      return patch === null ? (v) => at(v) >= 0 && compare(v, upper()) < 0 : (v) => at(v) === 0;
  }
}

/** Whether `version` is in `range`: true, false, or null when either can't be read. */
export function satisfies(version: string, range: string | null | undefined): boolean | null {
  const v = parse(version);
  if (!v) return null;
  if (!range || range.trim() === "" || range.trim() === "*") return true;
  let any = false;
  for (const alternative of range.split("||")) {
    // ("1.2.0 - 1.4.0": a hyphen range.)
    const hyphen = /^\s*(\S+)\s+-\s+(\S+)\s*$/.exec(alternative);
    const parts = hyphen ? [`>=${hyphen[1]}`, `<=${hyphen[2]}`] : alternative.trim().split(/\s+(?=[<>=^~v\d*xX])/).map((p) => p.replace(/\s+/g, ""));
    const tests = parts.map(comparator);
    if (tests.some((t) => t === null)) return null;
    if (tests.every((t) => t!(v))) any = true;
  }
  return any;
}
