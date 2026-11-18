/**
 * What a store listing's name, description or keywords may not say
 * (amendment 45): that the plugin carries the store's mark. ("Zeron"
 * anywhere is nobody's concern: the owner's ruling, contract fa75e5a40.) One
 * rule for a device's check and the edge: a port of the engine's
 * crates/engine/src/plugins/mark.rs, reading every list from the shared data
 * file (docs/plugin-store-data.json: `mark_claims`, `keyword_claims`,
 * `listing_causes`), whose `about` texts state each step, and held to the
 * shared listing cases (docs/plugin-package-vectors.json).
 *
 * Text is read as code points throughout: `folded` (NFKC, lowercase, NFKD
 * without nonspacing marks, then without default-ignorables and enclosing
 * marks); a word is spelled letter by letter with the shared look-alikes.
 *
 * - A name refuses every mark word, glyph and other-language word.
 * - A description refuses a mark word only when said of the plugin itself:
 *   beside a store word (4, no negation lifts it); alone as a sentence or
 *   the text's first word (1); directly after a subject word (3); within two
 *   words of a word for the plugin (2). A negation directly before lifts 1–3.
 * - A keyword may not say the mark.
 */
import { STORE_DATA } from "./data";

type Text = readonly string[];
type Span = readonly [number, number];

interface OtherWord {
  readonly word: string;
  readonly description: "folded" | "accented" | "never";
  readonly with_kana?: boolean;
  readonly before?: readonly string[];
}
interface WordList {
  readonly substring: boolean;
  readonly words: readonly string[];
}

const MARKS = STORE_DATA.mark_claims as unknown as {
  readonly glyphs: readonly string[];
  readonly words: readonly string[];
  readonly lookalikes: Readonly<Record<string, readonly string[]>>;
  readonly pairs: Readonly<Record<string, readonly string[]>>;
  readonly interchangeable: readonly (readonly string[])[];
  readonly max_spacing: number;
  readonly languages: Readonly<Record<string, { readonly substring: boolean; readonly words: readonly OtherWord[] }>>;
  readonly description: {
    readonly ambiguous: readonly string[];
    readonly articles: readonly string[];
    readonly by_words: readonly string[];
    readonly conjunctions: readonly string[];
    readonly list_marks: readonly string[];
    readonly negations: readonly string[];
    readonly negations_after: readonly string[];
    readonly authorities: readonly string[];
    readonly certified_words: readonly string[];
    readonly object_words: readonly string[];
    readonly plugin_words: Readonly<Record<string, WordList>>;
    readonly predicate_before: readonly string[];
    readonly relation_after: readonly string[];
    readonly relation_before_zh: readonly string[];
    readonly relation_words: readonly string[];
    readonly sentence_ends: readonly string[];
    readonly store_words: readonly string[];
    readonly subject_words: readonly string[];
  };
};
const MARK_GLYPHS = MARKS.glyphs;
const MARK_WORDS = MARKS.words;
const LOOKALIKES = MARKS.lookalikes;
const PAIRS = MARKS.pairs;
const INTERCHANGEABLE = MARKS.interchangeable;
const MAX_SPACING = MARKS.max_spacing;
const OTHER_LANGUAGES: readonly (readonly [boolean, readonly OtherWord[]])[] = Object.values(MARKS.languages).map((l) => [l.substring, l.words] as const);
const AMBIGUOUS = MARKS.description.ambiguous;
const NEGATIONS = MARKS.description.negations;
const PLUGIN_WORDS: readonly WordList[] = Object.values(MARKS.description.plugin_words);
const SENTENCE_ENDS = new Set(MARKS.description.sentence_ends);
const STORE_WORDS = MARKS.description.store_words;
const SUBJECT_WORDS = MARKS.description.subject_words;
const ARTICLES = MARKS.description.articles;
const BY_WORDS = MARKS.description.by_words;
const CONJUNCTIONS = MARKS.description.conjunctions;
const LIST_MARKS = new Set(MARKS.description.list_marks);
const NEGATIONS_AFTER = MARKS.description.negations_after;
const OBJECT_WORDS = MARKS.description.object_words;
const AUTHORITIES = MARKS.description.authorities;
const PREDICATE_BEFORE = MARKS.description.predicate_before;
const RELATION_AFTER = MARKS.description.relation_after;
const RELATION_BEFORE_ZH = MARKS.description.relation_before_zh;
const RELATION_WORDS = MARKS.description.relation_words;

const CERTIFIED_WORDS = MARKS.description.certified_words;
const CAUSES = (STORE_DATA as unknown as { listing_causes: { max_word_chars: number; written_max_words: number } }).listing_causes;
const WRITTEN_MAX_WORDS = CAUSES.written_max_words;

const SEPARATOR = /^[\p{White_Space}\u{feff}\p{P}]$/u;
const ANY_WORD = /^[\p{L}\p{N}]$/u;
const ASCII_WORD = /^[A-Za-z0-9]$/;
const ASCII = /^[\x00-\x7f]*$/;
const IGNORABLE = /[\p{Default_Ignorable_Code_Point}\p{Me}]/gu;
const WHITE_SPACE = /^\p{White_Space}$/u;

/** How a candidate word is held: a mark word; a word that is also an
 * ordinary word (a name's only, else only where that can't be); a word that
 * alone isn't the mark. */
type Kind = "mark" | "name_only" | "ambiguous";

/** `text` as the rules read it before ignorables go: NFKC, lowercase, then
 * NFKD without its nonspacing marks. */
export const markText = (text: string): string => text.normalize("NFKC").toLowerCase().normalize("NFKD").replace(/\p{Mn}/gu, "");
/** `markText` without anything drawn as nothing, nor enclosing marks. */
const folded = (text: string): string[] => [...markText(text).replace(IGNORABLE, "")];
/** NFKC and lowercase, keeping accents, ignorables and enclosing marks gone. */
const keptAccents = (text: string): string[] => [...text.normalize("NFKC").toLowerCase().replace(IGNORABLE, "")];

const separator = (c: string | undefined): boolean => c !== undefined && SEPARATOR.test(c);
const asciiWord = (c: string | undefined): boolean => c !== undefined && ASCII_WORD.test(c);
const anyWord = (c: string | undefined): boolean => c !== undefined && ANY_WORD.test(c);
const isAscii = (text: string): boolean => ASCII.test(markText(text));
const apostrophe = (c: string | undefined): boolean => c === "'" || c === "\u{2019}" || c === "\u{2bc}";
const cp = (c: string): number => c.codePointAt(0)!;
const han = (c: string): boolean => {
  const n = cp(c);
  return (n >= 0x3400 && n <= 0x4dbf) || (n >= 0x4e00 && n <= 0x9fff) || (n >= 0xf900 && n <= 0xfaff) || (n >= 0x20000 && n <= 0x3134f);
};
const kana = (n: number): boolean => (n >= 0x3040 && n <= 0x30ff) || (n >= 0x31f0 && n <= 0x31ff) || (n >= 0xff66 && n <= 0xff9f);
const hasKana = (text: string): boolean => [...text].some((c) => kana(cp(c)));
const eq = (a: Text, b: Text): boolean => a.length === b.length && a.every((c, i) => c === b[i]);
const startsWith = (text: Text, at: number, word: Text): boolean => word.every((c, n) => text[at + n] === c);
const endsWith = (text: Text, word: Text): boolean => word.length <= text.length && startsWith(text, text.length - word.length, word);

const lookalikesOf = (letter: string): Set<string> => new Set([letter, ...(LOOKALIKES[letter] ?? [])]);
/** The table's letter `c` passes for, if it's one of its look-alikes. */
const readsAs = (c: string): string => {
  if (c in LOOKALIKES) return c;
  for (const [letter, chars] of Object.entries(LOOKALIKES)) if (chars.includes(c)) return letter;
  return c;
};
const CLASSES = new Map<string, Set<string>>();
const classOf = (letter: string): Set<string> => {
  let found = CLASSES.get(letter);
  if (!found) {
    const base = readsAs(letter);
    found = lookalikesOf(base);
    found.add(letter);
    for (const [a, b] of INTERCHANGEABLE) {
      const other = a === base ? b : b === base ? a : undefined;
      if (other !== undefined) for (const c of lookalikesOf(other)) found.add(c);
    }
    CLASSES.set(letter, found);
  }
  return found;
};

/** Where `spelling` (from its `i`th character) ends when it matches `text`
 * from `at`, with no ASCII letter or digit right after. */
const spellEnd = (text: Text, at: number, spelling: Text, i = 0): number | undefined => {
  if (i === spelling.length) return asciiWord(text[at]) ? undefined : at;
  const letter = spelling[i]!;
  const gaps = (from: number): number[] => {
    const out: number[] = [];
    for (let n = from; n < text.length && separator(text[n]) && !apostrophe(text[n]); n++) out.push(n + 1);
    return out;
  };
  if (letter === " " || letter === "~") {
    for (const next of letter === "~" ? [at, ...gaps(at)] : gaps(at)) {
      const end = spellEnd(text, next, spelling, i + 1);
      if (end !== undefined) return end;
    }
    return undefined;
  }
  if (letter === "'") return apostrophe(text[at]) ? spellEnd(text, at + 1, spelling, i + 1) : undefined;
  if (text[at] !== undefined && classOf(letter).has(text[at]!)) {
    const end = spellEnd(text, at + 1, spelling, i + 1);
    if (end !== undefined) return end;
  }
  for (const pair of PAIRS[letter] ?? []) {
    const chars = [...pair];
    if (chars.every((p, n) => text[at + n] !== undefined && lookalikesOf(p).has(text[at + n]!))) {
      const end = spellEnd(text, at + chars.length, spelling, i + 1);
      if (end !== undefined) return end;
    }
  }
  return undefined;
};

/** Where each whole match of `words` in `text` ends. */
const ends = (text: Text, words: string): Set<number> => {
  const spelling = [...markText(words)];
  const found = new Set<number>();
  for (let at = 0; at < text.length; at++) {
    if (asciiWord(text[at - 1])) continue;
    const end = spellEnd(text, at, spelling);
    if (end !== undefined) found.add(end);
  }
  return found;
};
const spelled = (text: Text, words: string): boolean => ends(text, words).size > 0;

/** `text` with each run of single characters, apart by 1 to `max_spacing`
 * separators, read as one word. */
const unspaced = (text: Text): string[] => {
  const out: string[] = [];
  let at = 0;
  while (at < text.length) {
    if (!separator(text[at]) && (at === 0 || separator(text[at - 1]))) {
      const run = [text[at]!];
      let last = at;
      for (;;) {
        let gap = 0;
        while (last + 1 + gap < text.length && separator(text[last + 1 + gap])) gap++;
        const next = last + 1 + gap;
        if (gap < 1 || gap > MAX_SPACING || next >= text.length || (text[next + 1] !== undefined && !separator(text[next + 1]))) break;
        run.push(text[next]!);
        last = next;
      }
      if (run.length > 1) {
        out.push(...run);
        at = last + 1;
        continue;
      }
    }
    out.push(text[at]!);
    at++;
  }
  return out;
};

/** The words of `text`: runs of non-separators, each Han ideograph a word. */
const wordsOf = (text: Text): Span[] => {
  const words: Span[] = [];
  let start: number | undefined;
  text.forEach((c, at) => {
    if (separator(c) || han(c)) {
      if (start !== undefined) words.push([start, at]);
      start = undefined;
      if (han(c)) words.push([at, at + 1]);
    } else if (start === undefined) start = at;
  });
  if (start !== undefined) words.push([start, text.length]);
  return words;
};

/** Where `wanted` is in `text`: anywhere, or whole (no letter or digit of
 * any script beside it). */
const spans = (text: Text, wanted: Text, substring: boolean): Span[] => {
  if (wanted.length === 0) return [];
  const out: Span[] = [];
  for (let at = 0; at < text.length; at++) {
    if (startsWith(text, at, wanted) && (substring || (!anyWord(text[at - 1]) && !anyWord(text[at + wanted.length])))) out.push([at, at + wanted.length]);
  }
  return out;
};

/** A listed word, and whether it's a stem (a trailing `*`: any ending). */
const listed = (word: string): readonly [string, boolean] => (word.endsWith("*") ? [word.slice(0, -1), true] : [word, false]);

/** Where `word`'s letters, each of its class or a pair, end from `at`. */
const lettersEnd = (text: Text, at: number, word: Text, i = 0): number | undefined => {
  if (i === word.length) return at;
  const letter = word[i]!;
  if (text[at] !== undefined && classOf(letter).has(text[at]!)) {
    const end = lettersEnd(text, at + 1, word, i + 1);
    if (end !== undefined) return end;
  }
  for (const pair of PAIRS[letter] ?? []) {
    const chars = [...pair];
    if (chars.every((p, n) => text[at + n] !== undefined && lookalikesOf(p).has(text[at + n]!))) {
      const end = lettersEnd(text, at + chars.length, word, i + 1);
      if (end !== undefined) return end;
    }
  }
  return undefined;
};

/** Where a listed word is spelled with the look-alikes: anywhere, or whole;
 * a stem with any ending. */
const spelledSpans = (text: Text, word: Text, substring: boolean, stem: boolean): Span[] => {
  if (word.length === 0) return [];
  const out: Span[] = [];
  for (let at = 0; at < text.length; at++) {
    if (!substring && anyWord(text[at - 1])) continue;
    let end = lettersEnd(text, at, word);
    if (end === undefined) continue;
    if (stem) {
      while (anyWord(text[end])) end++;
      out.push([at, end]);
    } else if (substring || !anyWord(text[end])) out.push([at, end]);
  }
  return out;
};

/** Whether `word` is the only CJK in `text`. */
const loneCjk = (text: Text, word: Text): boolean => {
  const cjk = (c: string) => {
    const n = cp(c);
    return han(c) || kana(n) || (n >= 0x1100 && n <= 0x11ff) || (n >= 0x3130 && n <= 0x318f) || (n >= 0xac00 && n <= 0xd7af);
  };
  return text.filter(cjk).length === word.length && word.every(cjk);
};

/** Whether a word of a text is `word`. */
const sameWord = (text: Text, word: Text): boolean => eq(text, word);

/** A reading of a description: its text, its words, and how a listed word
 * is put in its form. */
class Reading {
  readonly words: Span[];
  constructor(
    readonly text: Text,
    readonly form: (word: string) => string[]
  ) {
    this.words = wordsOf(text);
  }

  covers([start, end]: Span): readonly [number, number] | undefined {
    const first = this.words.findIndex((w) => w[1] > start);
    let last = -1;
    for (let n = this.words.length - 1; n >= 0; n--) {
      if (this.words[n]![0] < end) {
        last = n;
        break;
      }
    }
    return first !== -1 && last !== -1 && first <= last ? [first, last] : undefined;
  }

  sentenceEndIn(from: number, to: number): boolean {
    if (from >= to) return false;
    for (let n = from; n < to; n++) if (SENTENCE_ENDS.has(this.text[n]!)) return true;
    return false;
  }

  private phraseWords(phrase: string): string[][] {
    const text = this.form(phrase);
    return wordsOf(text).map(([a, b]) => text.slice(a, b));
  }

  beforeIs(at: number, phrase: string): boolean {
    const parts = this.phraseWords(phrase);
    const wanted = parts.length;
    if (wanted === 0 || at < wanted || this.sentenceEndIn(this.words[at - wanted]![0], this.words[at]![0])) return false;
    return parts.every((part, n) => {
      const [a, b] = this.words[at - wanted + n]!;
      return sameWord(this.text.slice(a, b), part);
    });
  }

  afterIs(at: number, phrase: string): boolean {
    const parts = this.phraseWords(phrase);
    const wanted = parts.length;
    if (wanted === 0 || at + wanted >= this.words.length || this.sentenceEndIn(this.words[at]![1], this.words[at + wanted]![1])) return false;
    return parts.every((part, n) => {
      const [a, b] = this.words[at + 1 + n]!;
      return sameWord(this.text.slice(a, b), part);
    });
  }

  saidOfPlugin(span: Span, ambiguous: boolean): boolean {
    return this.saidOf(span, ambiguous ? "ambiguous" : "mark", !ambiguous);
  }

  /** Whether word `at` is one of `list`. */
  wordIs(at: number, list: readonly string[]): boolean {
    const [a, b] = this.words[at]!;
    const word = this.text.slice(a, b);
    return list.some((w) => sameWord(word, this.form(w)));
  }

  /** The sentence holding `span`: (from, to). */
  sentence(span: Span): readonly [number, number] {
    let from = 0;
    for (let n = span[0] - 1; n >= 0; n--) {
      if (SENTENCE_ENDS.has(this.text[n]!)) {
        from = n + 1;
        break;
      }
    }
    let to = this.text.length;
    for (let n = span[1]; n < this.text.length; n++) {
      if (SENTENCE_ENDS.has(this.text[n]!)) {
        to = n;
        break;
      }
    }
    return [from, to];
  }

  saidOf(span: Span, kind: Kind, firstWord: boolean): boolean {
    const covered = this.covers(span);
    if (!covered) return false;
    const [first, last] = covered;
    const [from, to] = this.sentence(span);
    const inSentence = (n: number) => this.words[n]![0] >= from && this.words[n]![1] <= to;
    // 4: beside "the store", "the plugin store". No negation lifts it.
    if (STORE_WORDS.some((w) => this.beforeIs(first, w) || this.afterIs(last, w))) return true;
    // A negation lifts the rest: within four words before it in its
    // sentence, inside its word (非公式), or after it in Japanese and Korean.
    const inside = this.text.slice(this.words[first]![0], span[0]);
    let negatedBefore = false;
    for (let n = Math.max(0, first - 4); n < first; n++) if (inSentence(n) && this.wordIs(n, NEGATIONS)) negatedBefore = true;
    const after = this.text.slice(span[1], to);
    if (
      negatedBefore ||
      NEGATIONS.some((n) => inside.length > 0 && endsWith(inside, this.form(n))) ||
      NEGATIONS_AFTER.some((n) => spans(after, this.form(n), true).length > 0)
    ) {
      return false;
    }
    const pluginBefore =
      first > 0 &&
      inSentence(first - 1) &&
      (() => {
        const [a, b] = this.words[first - 1]!;
        const word = this.text.slice(a, b);
        return PLUGIN_WORDS.some(({ substring, words }) =>
          words.some((entry) => {
            const [w, stem] = listed(entry);
            const wanted = this.form(w);
            return substring ? spans(word, wanted, true).length > 0 : eq(word, wanted) || (stem && startsWith(word, 0, wanted));
          })
        );
      })();
    // A word that gives it an object ("verified against Node 22"), or "by"
    // and someone other than the publisher or an authority ("verified by
    // GitHub"): a test, or someone else's thing. (Not after a word for the
    // plugin: "Cliente oficial para Git": "para Git" is the client's.)
    const next = last + 1 < this.words.length && inSentence(last + 1) && !pluginBefore ? last + 1 : undefined;
    if (next !== undefined && this.wordIs(next, OBJECT_WORDS)) return false;
    if (next !== undefined && this.wordIs(next, BY_WORDS)) {
      let who = next + 1;
      while (who < this.words.length && inSentence(who) && this.wordIs(who, ARTICLES)) who++;
      return who < this.words.length && inSentence(who) && this.wordIs(who, AUTHORITIES);
    }
    // Alone as a sentence: any of them.
    if (this.alone(covered, span)) return true;
    const afterConjunction = first > 0 && inSentence(first - 1) && this.wordIs(first - 1, CONJUNCTIONS);
    let listedBefore = false;
    for (let n = span[0] - 1; n >= from; n--) {
      const c = this.text[n]!;
      if (WHITE_SPACE.test(c)) continue;
      listedBefore = LIST_MARKS.has(c);
      break;
    }
    const predicateBefore =
      afterConjunction || listedBefore || (first > 0 && inSentence(first - 1) && (this.wordIs(first - 1, PREDICATE_BEFORE) || this.wordIs(first - 1, SUBJECT_WORDS)));
    let atEnd = true;
    for (let n = last + 1; n < this.words.length; n++) if (inSentence(n)) atEnd = false;
    const subject = first > 0 && inSentence(first - 1) && SUBJECT_WORDS.some((w) => this.beforeIs(first, w));
    let firstInText = true;
    for (let n = 0; n < first; n++) {
      const [a, b] = this.words[n]!;
      if (!(this.wordIs(n, ARTICLES) || !this.text.slice(a, b).some((c) => anyWord(c)))) firstInText = false;
    }
    switch (kind) {
      case "mark":
        // Ending its sentence ("Safe, fast and verified."), first (articles
        // and symbols aside), or after a word that says it of the subject.
        return (atEnd && predicateBefore) || (firstWord && firstInText) || subject || this.nearPlugin(covered, span, false);
      case "name_only": {
        // Also an ordinary word: only where that can't be.
        const conjunctionAfter = last + 1 < this.words.length && inSentence(last + 1) && this.wordIs(last + 1, CONJUNCTIONS);
        return subject || (firstWord && firstInText && conjunctionAfter) || (atEnd && (afterConjunction || pluginBefore));
      }
      case "ambiguous":
        return (atEnd && (listedBefore || afterConjunction)) || this.nearPlugin(covered, span, true);
    }
  }

  /** Rule 2: a word for the plugin (or, for a mark word, a word that says
   * it's certified) after it within two words with no relation between, or
   * directly before it; not when the phrase is someone else's. */
  nearPlugin([first, last]: readonly [number, number], span: Span, ambiguous: boolean): boolean {
    const neighbours: WordList[] = [...PLUGIN_WORDS, { substring: true, words: ambiguous ? [] : CERTIFIED_WORDS }];
    return neighbours.some(({ substring, words }) =>
      words.some((entry) => {
        const [word, stem] = listed(entry);
        const wanted = this.form(word);
        const found = substring ? spans(this.text, wanted, true) : this.phraseSpans(wanted, stem);
        return found.some((other) => {
          const around = this.covers(other);
          if (!around) return false;
          const [oFirst, oLast] = around;
          let near: boolean;
          if (oFirst > last) {
            const between = this.text.slice(span[1], other[0]);
            let related = RELATION_AFTER.some((r) => spans(between, this.form(r), true).length > 0);
            for (let n = last + 1; n < oFirst; n++) if (this.wordIs(n, RELATION_WORDS)) related = true;
            near = oFirst - last - 1 <= 2 && !this.sentenceEndIn(span[1], other[0]) && !related;
          } else if (first > oLast) {
            near = first - oLast - 1 === 0 && !this.sentenceEndIn(other[1], span[0]);
          } else near = true;
          return near && !this.someoneElses(Math.min(first, oFirst), Math.min(span[0], other[0]), other[1]);
        });
      })
    );
  }

  /** Whether the phrase starting at word `start` is someone else's: a
   * relation word before it (articles aside, at most two), a ZH relation
   * verb right before, or a ja/ko particle right after the thing. */
  someoneElses(start: number, startAt: number, thingEnd: number): boolean {
    let at = start;
    let skipped = 0;
    while (at > 0 && !this.sentenceEndIn(this.words[at - 1]![1], this.words[at]![0])) {
      at -= 1;
      if (this.wordIs(at, RELATION_WORDS)) return true;
      if (!this.wordIs(at, ARTICLES) || skipped === 2) break;
      skipped += 1;
    }
    let end = startAt;
    while (end > 0 && separator(this.text[end - 1])) end--;
    const before = this.text.slice(0, end);
    let begin = thingEnd;
    while (begin < this.text.length && separator(this.text[begin])) begin++;
    const after = this.text.slice(begin);
    return RELATION_BEFORE_ZH.some((r) => endsWith(before, this.form(r))) || RELATION_AFTER.some((r) => startsWith(after, 0, this.form(r)));
  }

  /** Whether every word of the sentence holding `span` is one it covers. */
  alone([first, last]: readonly [number, number], span: Span): boolean {
    let from = 0;
    for (let n = span[0] - 1; n >= 0; n--) {
      if (SENTENCE_ENDS.has(this.text[n]!)) {
        from = n + 1;
        break;
      }
    }
    let to = this.text.length;
    for (let n = span[1]; n < this.text.length; n++) {
      if (SENTENCE_ENDS.has(this.text[n]!)) {
        to = n;
        break;
      }
    }
    return this.words.every((w, n) => !(w[0] >= from && w[1] <= to) || (n >= first && n <= last));
  }

  /** Where a word or phrase of spaced words is, whole word by word (its last
   * word a stem: any ending). */
  phraseSpans(phrase: Text, stem: boolean): Span[] {
    const parts = wordsOf(phrase).map(([a, b]) => phrase.slice(a, b));
    if (parts.length === 0 || parts.length > this.words.length) return [];
    const out: Span[] = [];
    for (let at = 0; at <= this.words.length - parts.length; at++) {
      const fits = parts.every((part, n) => {
        const [a, b] = this.words[at + n]!;
        const word = this.text.slice(a, b);
        return sameWord(word, part) || (stem && n + 1 === parts.length && startsWith(word, 0, part));
      });
      if (fits && !this.sentenceEndIn(this.words[at]![0], this.words[at + parts.length - 1]![1])) {
        out.push([this.words[at]![0], this.words[at + parts.length - 1]![1]]);
      }
    }
    return out;
  }
}

/** The mark word written whole (ASCII boundaries) in `plain`, if any. */
const writtenWord = (plain: Text): string | undefined => {
  const joined = plain.join("");
  return MARK_WORDS.find((word) => {
    for (let at = joined.indexOf(word); at !== -1; at = joined.indexOf(word, at + 1)) {
      if (!asciiWord(joined[at - 1]) && !asciiWord(joined[at + word.length])) return true;
    }
    return false;
  });
};

/** A name's mark: any glyph, any mark word, each other-language word. */
const nameMark = (text: string): string | undefined => {
  const plain = folded(text);
  const glyph = plain.find((c) => MARK_GLYPHS.includes(c));
  if (glyph !== undefined) return glyph;
  const written = writtenWord(plain);
  if (written !== undefined) return written;
  const readings = [plain, unspaced(plain)];
  const word = MARK_WORDS.find((w) => readings.some((r) => spelled(r, w)));
  if (word !== undefined) return word;
  // A word that alone isn't the mark, within two words of a word for the
  // plugin, or the whole name.
  for (const ambiguous of AMBIGUOUS) {
    const wanted = folded(ambiguous);
    const said = readings.some((r) => {
      const reading = new Reading(r, folded);
      return spans(reading.text, wanted, true).some((span) => reading.saidOf(span, "ambiguous", false));
    });
    if (said) return ambiguous;
  }
  // In the store's other languages.
  const withKana = hasKana(text);
  for (const [substring, words] of OTHER_LANGUAGES) {
    for (const w of words) {
      const [listedWord, stem] = listed(w.word);
      if (w.with_kana && !withKana && !loneCjk(plain, folded(listedWord))) continue;
      const wanted = folded(listedWord);
      for (const r of readings) {
        for (const [start, end] of spelledSpans(r, wanted, substring, stem)) {
          const before =
            !w.before?.length ||
            (() => {
              let n = end;
              while (n < r.length && separator(r[n])) n++;
              const rest = r.slice(n);
              return w.before!.some((b) => startsWith(rest, 0, folded(b)));
            })();
          if (before) return stem ? r.slice(start, end).join("") : w.word;
        }
      }
    }
  }
  return undefined;
};

/** A description's mark: a glyph anywhere; a mark word only when said of
 * the plugin itself. */
const descriptionMark = (text: string): string | undefined => {
  const plainText = folded(text);
  const glyph = plainText.find((c) => MARK_GLYPHS.includes(c));
  if (glyph !== undefined) return glyph;
  for (const reading of [new Reading(plainText, folded), new Reading(unspaced(plainText), folded)]) {
    for (const word of MARK_WORDS) {
      const spelling = [...markText(word)];
      for (let at = 0; at < reading.text.length; at++) {
        if (asciiWord(reading.text[at - 1])) continue;
        const end = spellEnd(reading.text, at, spelling);
        if (end !== undefined && reading.saidOfPlugin([at, end], false)) return word;
      }
    }
    for (const [substring, words] of OTHER_LANGUAGES) {
      for (const w of words) {
        const [listedWord, stem] = listed(w.word);
        const lone = !!w.with_kana && loneCjk(reading.text, folded(listedWord));
        const said = (span: Span): boolean => {
          switch (w.description) {
            case "folded":
              return reading.saidOf(span, "mark", !substring);
            case "never":
              // 公式 alone among Latin: the mark, wherever it stands; 公式 and
              // 공식 beside other CJK: near the plugin, or ending a list; an
              // ordinary word too: only where that can't be.
              if (lone) return reading.saidOf(span, "mark", true);
              if (w.with_kana || (w.before?.length ?? 0) > 0) return reading.saidOf(span, "ambiguous", false);
              return reading.saidOf(span, "name_only", !substring);
            case "accented":
              return false;
          }
        };
        const found = spelledSpans(reading.text, folded(listedWord), substring, stem).find(said);
        if (found) return stem ? reading.text.slice(found[0], found[1]).join("") : w.word;
      }
    }
    // Not the mark alone, but near a word for the plugin (or beside the store).
    const ambiguous = [
      ...OTHER_LANGUAGES.flatMap(([, words]) => words.filter((w) => w.description === "never" && (w.with_kana || (w.before?.length ?? 0) > 0)).map((w) => w.word)),
      ...AMBIGUOUS
    ];
    for (const word of ambiguous) {
      if (spans(reading.text, folded(word), true).some((span) => reading.saidOfPlugin(span, true))) return word;
    }
  }
  // Words refused only as written with their accents.
  const kept = keptAccents(text);
  for (const reading of [new Reading(kept, keptAccents), new Reading(unspaced(kept), keptAccents)]) {
    for (const [substring, words] of OTHER_LANGUAGES) {
      for (const w of words.filter((w) => w.description === "accented")) {
        if (spans(reading.text, keptAccents(w.word), substring).some((span) => reading.saidOfPlugin(span, false))) return w.word;
      }
    }
  }
  return undefined;
};

/** What in `text` claims the store's mark, if anything: in a name
 * (`inName`) anywhere; in a description only when said of the plugin. */
export const markClaim = (text: string, inName = true): string | undefined => (inName ? nameMark(text) : descriptionMark(text));

/** What a keyword says that it may not: a part (or its parts joined) that
 * is a mark word in any language a keyword's letters can write. No badge
 * lifts it ("zeron" in a keyword is anyone's). */
export const keywordClaim = (keyword: string): string | undefined => {
  const parts = keyword.split(/[- ]/).filter((p) => p !== "").map(folded);
  const candidates = [...parts, parts.flat()];
  const whole = (text: Text, word: string) => spellEnd(text, 0, [...markText(word)]) === text.length;
  const words = [...MARK_WORDS, ...OTHER_LANGUAGES.flatMap(([, ws]) => ws.map((w) => listed(w.word)[0])).filter(isAscii)];
  return words.find((word) => candidates.some((part) => whole(part, word)));
};

/** A word as a refusal names it (`listing_causes`): without control,
 * format or other default-ignorable characters, at most `max_word_chars`
 * characters; undefined when nothing is left. */
export const problemWord = (text: string | undefined): string | undefined => {
  if (text === undefined) return undefined;
  const word = [...text].filter((c) => !/^[\p{Cc}\p{Cf}\p{Default_Ignorable_Code_Point}]$/u.test(c)).slice(0, CAUSES.max_word_chars).join("");
  return word === "" ? undefined : word;
};

/** The first character a name or description may not hold (amendment 31):
 * a control or format character, any other default-ignorable, a variation
 * selector but U+FE0F, or a U+FE0F not directly after an emoji. */
export const firstUnseen = (text: string): string | undefined => {
  let before: string | undefined;
  for (const c of text) {
    const selector = /^\p{Variation_Selector}$/u.test(c);
    const bad =
      /^[\p{Cc}\p{Cf}]$/u.test(c) ||
      (/^\p{Default_Ignorable_Code_Point}$/u.test(c) && !selector) ||
      (selector && c !== "\u{fe0f}") ||
      (c === "\u{fe0f}" && !(before !== undefined && /^\p{Emoji}$/u.test(before)));
    if (bad) return c;
    before = c;
  }
  return undefined;
};

/** Where a name or description says `claim`, as written (`listing_causes`):
 * the shortest, earliest run of its whitespace-separated words (at most
 * `written_max_words`) that read as a name claims the same, then without
 * its first character while it still does, then without its last;
 * undefined when no run does. */
export const asWritten = (text: string, claim: string): string | undefined => {
  const claims = (run: string) => nameMark(run) === claim;
  const chars = [...text];
  const words: [number, number][] = [];
  let start: number | undefined;
  chars.forEach((c, at) => {
    if (WHITE_SPACE.test(c)) {
      if (start !== undefined) words.push([start, at]);
      start = undefined;
    } else if (start === undefined) start = at;
  });
  if (start !== undefined) words.push([start, chars.length]);
  let run: string[] | undefined;
  for (let len = 1; len <= Math.min(words.length, WRITTEN_MAX_WORDS) && run === undefined; len++) {
    for (let at = 0; at + len <= words.length; at++) {
      const candidate = chars.slice(words[at]![0], words[at + len - 1]![1]);
      if (claims(candidate.join(""))) {
        run = candidate;
        break;
      }
    }
  }
  if (run === undefined) return undefined;
  while (run.length > 1 && claims(run.slice(1).join(""))) run = run.slice(1);
  while (run.length > 1 && claims(run.slice(0, -1).join(""))) run = run.slice(0, -1);
  return run.join("");
};
