/**
 * Syntax highlighting for transcript code blocks — a compact hand-rolled
 * tokenizer standing in for the desktop's tree-sitter stack. Tokens carry the
 * theme's syntax role names (`--rb-syntax-*`), so colors come from the theme
 * artifact on every variant; the mapping is stable even when the tokenizer
 * can't see what tree-sitter would (it is deliberately conservative: unsure
 * text stays plain rather than guessing a loud role).
 */

export type SyntaxRole =
  | "comment"
  | "keyword"
  | "string"
  | "stringSpecial"
  | "escape"
  | "number"
  | "boolean"
  | "type"
  | "function"
  | "property"
  | "constant"
  | "variableSpecial"
  | "operator"
  | "punctuation"
  | "tag"
  | "attribute"
  | "macro"
  // The roles the desktop's `HighlightKind` (crates/syntax/src/lib.rs:60-92)
  // carries that this tokenizer never emits: the union stays complete so a
  // future tokenizer upgrade maps one-for-one, and every member already has
  // its `.tk-*` CSS rule.
  | "typeBuiltin"
  | "constructor"
  | "functionBuiltin"
  | "variable"
  | "parameter"
  | "label"
  | "markupHeading"
  | "markupRaw"
  | "markupLink"
  | "markupReference"
  | "markupEmphasis"
  | "markupStrong"
  | "embedded"
  | "invalid";

export interface SyntaxToken {
  readonly text: string;
  /** null = plain text color. */
  readonly role: SyntaxRole | null;
}

interface LanguageSpec {
  readonly lineComments: readonly string[];
  readonly blockComments: readonly (readonly [string, string])[];
  readonly keywords: ReadonlySet<string>;
  /** `true` treats backtick strings as template literals (stringSpecial). */
  readonly templateString?: boolean;
}

const JS_KW = [
  "async", "await", "break", "case", "catch", "class", "const", "continue", "debugger", "default",
  "delete", "do", "else", "export", "extends", "finally", "for", "from", "function", "if", "import",
  "in", "instanceof", "let", "new", "of", "return", "static", "super", "switch", "this", "throw",
  "try", "typeof", "var", "void", "while", "with", "yield",
];
const TS_KW = [...JS_KW, "abstract", "as", "declare", "enum", "implements", "interface", "is", "keyof", "namespace", "override", "private", "protected", "public", "readonly", "satisfies", "type"];
const PY_KW = [
  "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
  "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda",
  "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield", "match",
];
const RUST_KW = [
  "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
  "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
  "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe",
  "use", "where", "while",
];
const C_LIKE_KW = [
  "auto", "break", "case", "catch", "class", "const", "continue", "default", "delete", "do",
  "else", "enum", "explicit", "export", "extern", "final", "finally", "for", "friend", "goto",
  "if", "inline", "interface", "long", "namespace", "new", "override", "private", "protected",
  "public", "register", "return", "short", "signed", "sizeof", "static", "struct", "switch",
  "template", "this", "throw", "throws", "try", "typedef", "typename", "union", "unsigned",
  "using", "virtual", "void", "volatile", "while", "sealed", "record", "var", "is", "null",
];
const GO_KW = [
  "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for",
  "func", "go", "goto", "if", "import", "interface", "map", "package", "range", "return", "select",
  "struct", "switch", "type", "var",
];
const BASH_KW = [
  "case", "do", "done", "elif", "else", "esac", "fi", "for", "function", "if", "in", "select",
  "then", "until", "while", "echo", "cd", "export", "local", "return", "set", "source", "shift",
];
const SQL_KW = [
  "select", "from", "where", "insert", "update", "delete", "join", "left", "right", "inner",
  "outer", "on", "group", "by", "order", "limit", "offset", "create", "table", "alter", "drop",
  "index", "values", "into", "set", "and", "or", "not", "null", "as", "distinct", "union", "all",
  "having", "exists", "case", "when", "then", "else", "end", "primary", "key", "references",
];
const RUBY_KW = [
  "alias", "and", "begin", "break", "case", "class", "def", "defined", "do", "else", "elsif",
  "end", "ensure", "false", "for", "if", "in", "module", "next", "nil", "not", "or", "redo",
  "rescue", "retry", "return", "self", "super", "then", "true", "undef", "unless", "until",
  "when", "while", "yield",
];

const C_LIKE: LanguageSpec = {
  lineComments: ["//"],
  blockComments: [["/*", "*/"]],
  keywords: new Set(C_LIKE_KW),
};

const LANGUAGES: Record<string, LanguageSpec> = {
  javascript: { ...C_LIKE, keywords: new Set(JS_KW), templateString: true },
  typescript: { ...C_LIKE, keywords: new Set(TS_KW), templateString: true },
  jsx: { ...C_LIKE, keywords: new Set(JS_KW), templateString: true },
  tsx: { ...C_LIKE, keywords: new Set(TS_KW), templateString: true },
  python: { lineComments: ["#"], blockComments: [['"""', '"""'], ["'''", "'''"]], keywords: new Set(PY_KW), templateString: true },
  rust: { ...C_LIKE, keywords: new Set(RUST_KW) },
  go: { ...C_LIKE, keywords: new Set(GO_KW), templateString: true },
  c: C_LIKE,
  cpp: C_LIKE,
  java: C_LIKE,
  csharp: C_LIKE,
  kotlin: { ...C_LIKE, keywords: new Set([...C_LIKE_KW, "fun", "val", "when", "object", "companion", "data", "sealed"]) },
  swift: { ...C_LIKE, keywords: new Set([...C_LIKE_KW, "func", "let", "var", "guard", "defer", "protocol", "extension", "some"]) },
  php: { lineComments: ["//", "#"], blockComments: [["/*", "*/"]], keywords: new Set([...C_LIKE_KW, "echo", "fn", "foreach", "as", "match"]) },
  ruby: { lineComments: ["#"], blockComments: [], keywords: new Set(RUBY_KW), templateString: true },
  bash: { lineComments: ["#"], blockComments: [], keywords: new Set(BASH_KW) },
  shell: { lineComments: ["#"], blockComments: [], keywords: new Set(BASH_KW) },
  sql: { lineComments: ["--"], blockComments: [["/*", "*/"]], keywords: new Set(SQL_KW) },
};

const ALIASES: Record<string, string> = {
  js: "javascript", mjs: "javascript", cjs: "javascript", javascriptreact: "jsx",
  ts: "typescript", mts: "typescript", cts: "typescript", typescriptreact: "tsx",
  py: "python", rs: "rust", golang: "go", "c++": "cpp", cxx: "cpp", hpp: "cpp", h: "c",
  cs: "csharp", "c#": "csharp", kt: "kotlin", kts: "kotlin", sh: "bash", zsh: "bash",
  shellscript: "bash", bash: "bash", rb: "ruby", yml: "yaml", md: "markdown",
};

const IDENT_RE = /[A-Za-z_][A-Za-z0-9_]*/y;
const NUMBER_RE = /0[xX][0-9a-fA-F_]+|0[bB][01_]+|0[oO][0-7_]+|\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][+-]?\d+)?/y;

const BOOLEANS = new Set(["true", "false", "null", "None", "True", "False", "nil", "undefined"]);

function push(tokens: SyntaxToken[], text: string, role: SyntaxRole | null): void {
  if (text.length === 0) {
    return;
  }
  const last = tokens[tokens.length - 1];
  if (last !== undefined && last.role === role) {
    tokens[tokens.length - 1] = { text: last.text + text, role };
    return;
  }
  tokens.push({ text, role });
}

/**
 * Tokenize `code` in `language` (a fence info string; aliases resolve). The
 * label arrives verbatim from the fence, so the lookup lowercases it — the
 * desktop's tree-sitter resolves case-insensitively the same way.
 */
export function highlightCode(code: string, language: string | null): SyntaxToken[] {
  const label = language?.toLowerCase() ?? null;
  const key = label === null ? null : (ALIASES[label] ?? label);
  const spec = key === null ? undefined : LANGUAGES[key];
  if (label === "json") {
    return highlightJson(code);
  }
  if (label === "yaml" || label === "yml") {
    return highlightYaml(code);
  }
  if (label === "html" || label === "xml" || label === "svg") {
    return highlightMarkup(code);
  }
  if (spec === undefined) {
    return highlightGeneric(code);
  }
  return highlightCLike(code, spec);
}

/** No language: comments and strings still read as structure, conservatively. */
function highlightGeneric(code: string): SyntaxToken[] {
  return highlightCLike(code, {
    lineComments: ["//", "#"],
    blockComments: [["/*", "*/"]],
    keywords: new Set(),
  });
}

function highlightCLike(code: string, spec: LanguageSpec): SyntaxToken[] {
  const tokens: SyntaxToken[] = [];
  const n = code.length;
  let i = 0;

  const matchLineComment = (): string | null => {
    for (const marker of spec.lineComments) {
      if (code.startsWith(marker, i)) {
        return marker;
      }
    }
    return null;
  };
  const matchBlockComment = (): readonly [string, string] | null => {
    for (const pair of spec.blockComments) {
      if (code.startsWith(pair[0], i)) {
        return pair;
      }
    }
    return null;
  };

  while (i < n) {
    const c = code[i]!;

    const lineComment = matchLineComment();
    if (lineComment !== null) {
      let end = code.indexOf("\n", i);
      if (end < 0) {
        end = n;
      }
      push(tokens, code.slice(i, end), "comment");
      i = end;
      continue;
    }
    const block = matchBlockComment();
    if (block !== null) {
      const close = code.indexOf(block[1], i + block[0].length);
      const end = close < 0 ? n : close + block[1].length;
      push(tokens, code.slice(i, end), "comment");
      i = end;
      continue;
    }

    if (c === '"' || c === "'" || c === "`") {
      const quote = c;
      const template = quote === "`" && spec.templateString === true;
      let j = i + 1;
      while (j < n) {
        if (code[j] === "\\") {
          j += 2;
          continue;
        }
        if (code[j] === quote) {
          j++;
          break;
        }
        if (quote !== "`" && code[j] === "\n") {
          // Most languages don't allow raw newlines in strings; stop so a
          // missing closer doesn't swallow the rest of the block.
          break;
        }
        j++;
      }
      push(tokens, code.slice(i, j), template ? "stringSpecial" : "string");
      i = j;
      continue;
    }

    NUMBER_RE.lastIndex = i;
    const num = NUMBER_RE.exec(code);
    if (num !== null && num[0].length > 0) {
      push(tokens, num[0], "number");
      i += num[0].length;
      continue;
    }

    IDENT_RE.lastIndex = i;
    const ident = IDENT_RE.exec(code);
    if (ident !== null && ident[0].length > 0) {
      const word = ident[0];
      let role: SyntaxRole | null = null;
      if (spec.keywords.has(word) || spec.keywords.has(word.toLowerCase())) {
        role = "keyword";
      } else if (BOOLEANS.has(word)) {
        role = "boolean";
      } else {
        let look = i + word.length;
        while (code[look] === " ") {
          look++;
        }
        const prev = i > 0 ? code[i - 1] : "";
        if (code[look] === "(" || code[look] === "<" && /^[a-z]/.test(word)) {
          role = "function";
        } else if (prev === ".") {
          role = "property";
        } else if (/^[A-Z]/.test(word)) {
          role = "type";
        } else if (/^[A-Z][A-Z0-9_]+$/.test(word)) {
          role = "constant";
        }
      }
      push(tokens, word, role);
      i += word.length;
      continue;
    }

    if ("(){}[]".includes(c)) {
      push(tokens, c, "punctuation");
      i++;
      continue;
    }
    if ("+-*/%=<>!&|^~?:".includes(c)) {
      push(tokens, c, "operator");
      i++;
      continue;
    }
    push(tokens, c, null);
    i++;
  }
  return tokens;
}

/** JSON: keys as properties, literals as constants, strings/numbers marked. */
function highlightJson(code: string): SyntaxToken[] {
  const tokens: SyntaxToken[] = [];
  let i = 0;
  while (i < code.length) {
    const c = code[i]!;
    if (c === '"') {
      let j = i + 1;
      while (j < code.length && code[j] !== '"') {
        j += code[j] === "\\" ? 2 : 1;
      }
      j = Math.min(j + 1, code.length);
      let look = j;
      while (code[look] === " " || code[look] === "\t") {
        look++;
      }
      push(tokens, code.slice(i, j), code[look] === ":" ? "property" : "string");
      i = j;
      continue;
    }
    NUMBER_RE.lastIndex = i;
    const num = NUMBER_RE.exec(code);
    if (num !== null && num[0].length > 0 && !/[A-Za-z_$]/.test(code[i - 1] ?? "")) {
      push(tokens, num[0], "number");
      i += num[0].length;
      continue;
    }
    const literal = /^(true|false|null)\b/y;
    literal.lastIndex = i;
    const lit = literal.exec(code);
    if (lit !== null) {
      push(tokens, lit[0], "boolean");
      i += lit[0].length;
      continue;
    }
    if ("{}[]".includes(c)) {
      push(tokens, c, "punctuation");
    } else if (":".includes(c)) {
      push(tokens, c, "operator");
    } else {
      push(tokens, c, null);
    }
    i++;
  }
  return tokens;
}

/** YAML: mapping keys as properties, comments, and scalars stay plain. */
function highlightYaml(code: string): SyntaxToken[] {
  const tokens: SyntaxToken[] = [];
  for (const line of code.split("\n")) {
    const hash = line.indexOf("#");
    const body = hash < 0 ? line : line.slice(0, hash);
    const keyMatch = /^(\s*)([^:\s][^:]*)(:)/.exec(body);
    if (keyMatch !== null) {
      const indent = keyMatch[1] ?? "";
      if (indent.length > 0) {
        push(tokens, indent, null);
      }
      push(tokens, keyMatch[2] ?? "", "property");
      push(tokens, ":", "operator");
      push(tokens, body.slice(keyMatch[0].length), null);
    } else {
      push(tokens, body, null);
    }
    if (hash >= 0) {
      push(tokens, line.slice(hash), "comment");
    }
    push(tokens, "\n", null);
  }
  return tokens;
}

/** HTML/XML: tag names, attributes, strings, comments. */
function highlightMarkup(code: string): SyntaxToken[] {
  const tokens: SyntaxToken[] = [];
  let i = 0;
  while (i < code.length) {
    if (code.startsWith("<!--", i)) {
      const close = code.indexOf("-->", i + 4);
      const end = close < 0 ? code.length : close + 3;
      push(tokens, code.slice(i, end), "comment");
      i = end;
      continue;
    }
    if (code[i] === "<" && /[A-Za-z/!]/.test(code[i + 1] ?? "")) {
      const tagMatch = /^<\/?[A-Za-z][A-Za-z0-9-]*/.exec(code.slice(i));
      if (tagMatch !== null) {
        push(tokens, tagMatch[0], "tag");
        i += tagMatch[0].length;
        let inTag = true;
        while (inTag && i < code.length) {
          const c = code[i]!;
          if (c === ">") {
            push(tokens, c, "punctuation");
            i++;
            inTag = false;
          } else if (c === '"' || c === "'") {
            let j = i + 1;
            while (j < code.length && code[j] !== c) {
              j++;
            }
            push(tokens, code.slice(i, Math.min(j + 1, code.length)), "string");
            i = Math.min(j + 1, code.length);
          } else if (/[A-Za-z-]/.test(c)) {
            const attr = /^[A-Za-z-]+/.exec(code.slice(i))![0];
            push(tokens, attr, "attribute");
            i += attr.length;
          } else {
            push(tokens, c, null);
            i++;
          }
        }
        continue;
      }
    }
    push(tokens, code[i]!, null);
    i++;
  }
  return tokens;
}

/** Split tokens into lines for row rendering (the newline ends its line). */
export function splitTokenLines(tokens: readonly SyntaxToken[]): SyntaxToken[][] {
  const lines: SyntaxToken[][] = [[]];
  for (const token of tokens) {
    let text = token.text;
    while (text.length > 0) {
      const nl = text.indexOf("\n");
      if (nl < 0) {
        lines[lines.length - 1]!.push({ text, role: token.role });
        break;
      }
      if (nl > 0) {
        lines[lines.length - 1]!.push({ text: text.slice(0, nl), role: token.role });
      }
      lines.push([]);
      text = text.slice(nl + 1);
    }
  }
  return lines;
}
