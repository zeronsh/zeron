// Families of shapes built to make a markdown renderer slow or large, each
// a function of the size wanted (about n characters). Most are the identity
// lane's, from its attack on the store's renderer (its F1 to F4 were found
// with them); the rest are this lane's own. md/growth.test.ts fits how each
// one's counted work and time grow with n, and md/heap.test.ts runs the
// heaviest under a small heap. Seeded, so a failure reproduces.

type Family = (n: number) => string;

const rep = (s: string, n: number) => s.repeat(Math.max(1, Math.floor(n / s.length)));

/** A generator of numbers in [0, 1) from a seed (the same seed, the same numbers). */
export function random(seed: number): () => number {
  let state = seed;
  return () => ((state = (Math.imul(state, 1103515245) + 12345) & 0x7fffffff), state / 0x80000000);
}

export const FAMILIES: Record<string, Family> = {
  // nesting
  "quotes deep 32": (n: number) => rep(`${"> ".repeat(32)}a\n`, n),
  "quotes deep 40 + blank": (n: number) => rep(`${">".repeat(40)}a\n\n`, n),
  "quotes lazy": (n: number) => rep("> > > > a\nb\n", n),
  "lists alt markers": (n: number) => rep("- a\n* b\n+ c\n1. d\n2) e\n", n),
  "lists nested 32": (n: number) => rep(`${"- ".repeat(32)}a\n`, n),
  "ordered nested 40": (n: number) => rep(`${"1. ".repeat(40)}a\n`, n),
  "lists nested then deep-indented lines": (n: number) => `${"- ".repeat(32)}a\n` + rep(`${" ".repeat(200)}b\n`, n),
  "lists nested then long blank lines": (n: number) => `${"- ".repeat(32)}a\n` + rep(`${" ".repeat(500)}\n${" ".repeat(64)}b\n`, n),
  "list markers then many spaces": (n: number) => rep(`${"- ".repeat(31)}-${" ".repeat(4000)}x\n`, n),
  "list markers tab": (n: number) => rep(`${"-\t".repeat(32)}${"x".repeat(2000)}\n`, n),
  "list markers tab rule-ish": (n: number) => rep(`${"-\t".repeat(32)}${" ".repeat(2000)}-\n`, n),
  "lists loose huge": (n: number) => rep("- a\n\n  b\n", n),
  "list items empty": (n: number) => rep("-\n", n),
  "lazy para in deep list": (n: number) => `${"- ".repeat(32)}a\n` + rep("b\n", n),
  // emphasis
  "* runs": (n: number) => rep("*", n),
  "*a runs": (n: number) => rep("*a", n),
  "_a_ mixed": (n: number) => rep("_a*b~c", n),
  "***a**b*": (n: number) => rep("***a**b* ", n),
  "open emph many": (n: number) => rep("*a ", n),
  "close emph many": (n: number) => rep("a* ", n),
  "open then close": (n: number) => rep("*a ", n / 2) + rep("a* ", n / 2),
  "_ open then * close": (n: number) => rep("_a ", n / 2) + rep("a* ", n / 2),
  "varying run lengths": (n: number) => {
    let s = "";
    for (let k = 1; s.length < n; k = (k % 50) + 1) s += `a${"*".repeat(k)} ${"*".repeat(k)}b `;
    return s;
  },
  "varying run lengths ascending opener": (n: number) => {
    let s = "";
    let k = 1;
    while (s.length < n / 2) s += `${"*".repeat(k++)}a `;
    while (s.length < n) s += `a${"*".repeat(--k > 0 ? k : 1)} `;
    return s;
  },
  "rule of 3 trap": (n: number) => rep("**a ", n / 2) + rep("a*b* ", n / 2),
  "tilde mix": (n: number) => rep("~a ~~b ", n / 2) + rep("a~~ b~ ", n / 2),
  "flank punct": (n: number) => rep('"*"a', n),
  "under intraword": (n: number) => rep("a_b", n),
  // brackets
  "[ only": (n: number) => rep("[", n),
  "![ only": (n: number) => rep("![", n),
  "] only": (n: number) => rep("]", n),
  "[a]( unclosed": (n: number) => rep("[a](", n),
  "nested links": (n: number) => rep("[", n / 2) + rep("a](b)", n / 2),
  "nested images links": (n: number) => rep("![[", n / 2) + rep("a](b)]", n / 2),
  "dest nested parens": (n: number) => `[a](${rep("(", n / 2)}${rep(")", n / 2)})`,
  "dest parens many links": (n: number) => rep("[a](b(c)d)", n),
  "dest parens unbalanced many": (n: number) => rep("[a](b(c", n),
  "titles unclosed": (n: number) => rep('[a](b "c', n),
  "titles paren": (n: number) => rep("[a](b (c", n),
  "title far close": (n: number) => rep('[a](b "', n) + '")',
  "[a][ unclosed": (n: number) => rep("[a][", n),
  "[a][b] undefined": (n: number) => rep("[a][b]", n),
  "refs many defs": (n: number) => {
    let d = "";
    let u = "";
    for (let i = 0; d.length + u.length < n; i++) {
      d += `[l${i}]: /u${i} "t"\n`;
      u += `[x][l${i}] `;
    }
    return `${u}\n\n${d}`;
  },
  "ref long dest reused": (n: number) => `[l]: ${"a".repeat(n / 2)}\n\n` + rep("[l]", n / 2),
  "ref long dest& reused": (n: number) => `[l]: &${"a".repeat(n / 2)}\n\n` + rep("[l]", n / 2),
  "ref long title& reused": (n: number) => `[l]: u "&${"a".repeat(n / 2)}"\n\n` + rep("[l]", n / 2),
  "ref long https dest reused": (n: number) => `[l]: https://a.b/${"a".repeat(n / 2)}\n\n` + rep("[l]", n / 2),
  "img ref long title reused": (n: number) => `[l]: a.png "${"a".repeat(n / 2)}"\n\n` + rep("![l]", n / 2),
  "ref labels 999": (n: number) => rep(`[${"a".repeat(990)}]`, n),
  "ref labels 999 full": (n: number) => rep(`[a][${"a ".repeat(495)}]`, n),
  "defs unclosed title": (n: number) => rep('[a]: b\n"\n', n),
  "defs long each": (n: number) => rep(`[a]: ${"b".repeat(1000)} "${"c".repeat(1000)}\n`, n),
  "defs then setext": (n: number) => rep("[a]: b\n=\n", n),
  // code spans
  "backtick ladder": (n: number) => {
    let s = "";
    for (let k = 1; s.length < n; k++) s += `${"`".repeat(k)}a`;
    return s;
  },
  "backtick ladder desc": (n: number) => {
    let k = 1;
    while ((k * (k + 3)) / 2 < n) k++;
    let s = "";
    for (; k > 0; k--) s += `${"`".repeat(k)}a`;
    return s;
  },
  "backtick 1-2 alternating": (n: number) => rep("`a``b", n),
  "backtick escaped": (n: number) => rep("\\``a", n),
  "backtick unclosed big": (n: number) => rep("``` ", n),
  // autolinks / entities / html
  "< unclosed": (n: number) => rep("<a", n),
  "<https: autolinks": (n: number) => rep("<https://a.b/c> ", n),
  "<email": (n: number) => rep("<a@b.c> ", n),
  "bare https many": (n: number) => rep("https://a.b/c ", n),
  "bare https no space": (n: number) => rep("https://a.b/c", n),
  "bare https parens": (n: number) => `https://a.b/${rep("(", n / 2)}${rep(")", n / 2)}`,
  "bare https entity tail": (n: number) => rep("https://a.b&a;", n),
  "bare https ;;;": (n: number) => `https://a.b/${rep("a;", n)}`,
  "entities named": (n: number) => rep("&amp;&copy;&nbsp;&zzzz;", n),
  "entities numeric": (n: number) => rep("&#x1F600;&#0;&#xD800;&#99999999;", n),
  "entities long invalid": (n: number) => rep(`&${"a".repeat(40)};`, n),
  "html block tags": (n: number) => rep("<div>\n\n", n),
  "html comment unclosed": (n: number) => `<!--\n${rep("a\n", n)}`,
  "html kind7 attrs": (n: number) => `<a ${rep("b='x", n)}\n`,
  "html kind7 many attrs lines": (n: number) => rep(`<a ${'b="x" '.repeat(100)}>\n`, n),
  "inline html text": (n: number) => rep('<span class="x" onclick="y">a</span>', n),
  // tables
  "table wide 64 rows 1byte": (n: number) => `|${"a|".repeat(64)}\n|${"-|".repeat(64)}\n` + rep("a\n", n),
  "table wide 64 center rows": (n: number) => `|${"a|".repeat(64)}\n|${":-:|".repeat(64)}\n` + rep("a\n", n),
  "table rows many pipes": (n: number) => `|a|b|\n|-|-|\n` + rep(`${"|".repeat(1000)}\n`, n),
  "table escaped pipes": (n: number) => `|a|b|\n|-|-|\n` + rep(`${"\\|".repeat(500)}\n`, n),
  "table candidates fail": (n: number) => rep("a|b|c\n|-|-|\n", n),
  "table header mismatch para": (n: number) => rep(`${"|a".repeat(100)}\n${"|-".repeat(99)}\n`, n),
  "table 65 cols": (n: number) => `|${"a|".repeat(65)}\n|${"-|".repeat(65)}\n` + rep("a\n", n),
  // headings / breaks / fences
  "setext atx rule mix": (n: number) => rep("a\n===\n# b #\n---\n***\nc\n---\n", n),
  "empty headings": (n: number) => rep("#\n", n),
  "same headings (slug dedup)": (n: number) => rep("# a\n", n),
  "slug collision pre-seeded": (n: number) => {
    let s = "";
    for (let i = 1; s.length < n / 2; i++) s += `# a-${i}\n`;
    return s + rep("# a\n", n / 2);
  },
  "rules": (n: number) => rep("---\n", n),
  "fence unclosed": (n: number) => "```\n" + rep("a\n", n),
  "fences many": (n: number) => rep("```js\na\n```\n~~~\nb\n~~~\n", n),
  "fence info quotes": (n: number) => rep('```"<x y\'>\na\n```\n', n),
  "fence long opener": (n: number) => `${"`".repeat(n / 2)}\n` + rep("`\n", n / 2),
  "fences of growing length": (n: number) => {
    let s = "";
    for (let k = 3; s.length < n; k++) s += `${"`".repeat(k)}\n`;
    return s;
  },
  "hard breaks": (n: number) => rep("a  \nb\\\n", n),
  "trailing spaces then newline": (n: number) => rep(`a${" ".repeat(1000)}\n`, n),
  "tabs": (n: number) => rep("\t-\t>\t*\ta\n", n),
  "CR": (n: number) => rep("a\r> b\r- c\r", n),
  "CRLF": (n: number) => rep("a\r\n> b\r\n- c\r\n", n),
  "long line plain": (n: number) => "a".repeat(n),
  "long line spaces": (n: number) => " ".repeat(n - 1) + "a",
  "one paragraph many lines": (n: number) => rep("a\n", n),
  "blank lines": (n: number) => "\n".repeat(n),
  "tiny blocks": (n: number) => rep("a\n\n", n),
  "indented code": (n: number) => rep("    a\n", n),
  "NUL": (n: number) => rep("\0", n),
};
FAMILIES["deep list then blank lines"] = (n: number) => `${"- ".repeat(32)}a\n` + "\n".repeat(n);
FAMILIES["deep list then 1-char lazy"] = (n: number) => `${"- ".repeat(32)}a\n` + "b\n".repeat(n / 2);
FAMILIES["deep mixed quote list blanks"] = (n: number) => `${"> - ".repeat(16)}a\n` + `${"> ".repeat(1)}\n`.repeat(n / 3);
FAMILIES["empty list items"] = (n: number) => "-\n".repeat(n / 2);
FAMILIES["empty tasks"] = (n: number) => "- [ ]\n".repeat(n / 6);
FAMILIES["empty headings h1"] = (n: number) => "#\n".repeat(n / 2);
FAMILIES["quote 32 then blank"] = (n: number) => `${">".repeat(32)}\n\n`.repeat(n / 34);

// This lane's own, from its first tier (and the shapes that beat its estimate before).
FAMILIES["emphasis openers, then links"] = (n) => "*a ".repeat(n / 6) + "[a](b) ".repeat(n / 14);
FAMILIES["links inside unclosed brackets"] = (n) => "[".repeat(n / 2) + "[a](b)".repeat(n / 12);
FAMILIES["link tails, <unclosed"] = (n) => "[a](<".repeat(n / 5);
FAMILIES["* and _ alternating"] = (n) => "*_".repeat(n / 2);
FAMILIES["closers then openers"] = (n) => "a* *a ".repeat(n / 6);
FAMILIES["definitions with a title on the next line"] = (n) => '[a]: b\n"\n'.repeat(n / 9);
FAMILIES["a fake fence (a backtick in its info string)"] = (n) => "``` a`b\n" + "a\n=\n".repeat(n / 4);
FAMILIES["a sparse table 1,300 columns wide"] = (n) => `|${"a|".repeat(1300)}\n|${"-|".repeat(1300)}\n` + "|a|\n".repeat(n / 4);
FAMILIES["a markdown code block of link openers"] = (n) => "```markdown\n" + "[a](".repeat(n / 4) + "\n```\n";
FAMILIES["an ordinary README"] = (n) => rep("## A heading\n\nSome *words* and a [link](https://example.com), `code`.\n\n- one\n- two\n\n```js\nconst a = 1;\n```\n\n", n);
FAMILIES["prose and tables"] = (n) =>
  rep("## Usage\n\nThis plugin shows the cost of each chat in the sidebar, from your agents' own usage.\n\n| Setting | What it does |\n|---|---|\n| `limit` | The monthly limit. |\n| `warn` | When to warn you. |\n\n", n);

/** The pieces a seeded document is made of: every family's start, and markup on its own. */
const PIECES = [
  "*", "_", "~", "[", "]", "(", ")", "`", "<", ">", "!", "\\", "&", "#", "\n", "\n\n", " ", "  ", "a", "|", "-", '"', "'", ":",
  "<script>alert(1)</script>", "javascript:alert(1)", "https://x.y/", "](", "![", "[x]: ", "    ", "> ", "- ", "1. ", "```", "---", "&lt;", "&#0;", "\u0000", "\r", "\t",
];

/** A document made at random from pieces and short runs of every family: the same seed, the same document. */
export function seeded(seed: number): Family {
  return (n) => {
    const next = random(seed);
    const families = Object.values(FAMILIES);
    const parts: string[] = [];
    let length = 0;
    while (length < n) {
      const part = next() < 0.5 ? PIECES[Math.floor(next() * PIECES.length)] : families[Math.floor(next() * families.length)](64 + Math.floor(next() * 1024));
      parts.push(part);
      length += part.length;
    }
    return parts.join("").slice(0, n);
  };
}
