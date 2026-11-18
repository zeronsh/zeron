// For the tests: blocks and inlines written as CommonMark's reference
// renderer writes them (no policy: every link and picture as written), so
// the parser can be compared with the CommonMark and GFM examples. Never
// used to draw a page.

import { blocksOf, type Block, type Definition } from "./blocks.ts";
import { inlinesOf, plainText, unescape, type Inline } from "./inlines.ts";

const esc = (s: string) => s.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");

/** As commonmark.js's mdurl: percent-encode what isn't safe in an address, keeping `%XX`. */
function url(s: string): string {
  let out = "";
  const bytes = new TextEncoder();
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (c === "%" && /^[0-9a-fA-F]{2}$/.test(s.slice(i + 1, i + 3))) {
      out += s.slice(i, i + 3);
      i += 2;
    } else if (/^[A-Za-z0-9;/?:@&=+$,\-_.!~*'()#]$/.test(c)) out += c;
    else {
      const cp = s.codePointAt(i)!;
      const ch = String.fromCodePoint(cp);
      if (ch.length === 2) i++;
      for (const b of bytes.encode(cp >= 0xd800 && cp <= 0xdfff ? "�" : ch)) out += `%${b.toString(16).toUpperCase().padStart(2, "0")}`;
    }
  }
  return out;
}

function inline(list: Inline[]): string {
  let out = "";
  for (const n of list) {
    switch (n.kind) {
      case "text":
        out += esc(n.text);
        break;
      case "code":
        out += `<code>${esc(n.text)}</code>`;
        break;
      case "softbreak":
        out += "\n";
        break;
      case "hardbreak":
        out += "<br />\n";
        break;
      case "em":
      case "strong":
      case "del":
        out += `<${n.kind}>${inline(n.children)}</${n.kind}>`;
        break;
      case "link":
        out += `<a href="${esc(url(n.destination))}"${n.title !== null ? ` title="${esc(n.title)}"` : ""}>${inline(n.children)}</a>`;
        break;
      case "image":
        out += `<img src="${esc(url(n.destination))}" alt="${esc(plainText(n.children))}"${n.title !== null ? ` title="${esc(n.title)}"` : ""} />`;
        break;
    }
  }
  return out;
}

function blocks(list: Block[], d: Map<string, Definition>, tight = false): string {
  let out = "";
  for (const b of list) {
    switch (b.kind) {
      case "paragraph":
        out += tight ? `${inline(inlinesOf(b.text, d).inlines)}${b === list[list.length - 1] ? "" : "\n"}` : `<p>${inline(inlinesOf(b.text, d).inlines)}</p>\n`;
        break;
      case "heading":
        out += `<h${b.level}>${inline(inlinesOf(b.text, d).inlines)}</h${b.level}>\n`;
        break;
      case "code": {
        const lang = unescape(b.info).split(/[ \t]/, 1)[0];
        out += `<pre><code${lang ? ` class="language-${esc(lang)}"` : ""}>${esc(b.text)}</code></pre>\n`;
        break;
      }
      case "rule":
        out += "<hr />\n";
        break;
      case "html":
        // (CommonMark's reference writes it through; the store writes its characters, escaped.)
        out += `${b.text}\n`;
        break;
      case "quote":
        out += `<blockquote>\n${blocks(b.children, d)}</blockquote>\n`;
        break;
      case "list": {
        const tag = b.ordered ? "ol" : "ul";
        out += `<${tag}${b.ordered && b.start !== 1 ? ` start="${b.start}"` : ""}>\n`;
        for (const item of b.items) {
          let inner = item.task !== null ? `<input disabled="" type="checkbox"${item.task ? ' checked=""' : ""}/>\n` : "";
          const body = blocks(item.children, d, b.tight);
          if (item.children.length > 0 && (!b.tight || item.children[0].kind !== "paragraph")) inner += `\n${body}`;
          else inner += body;
          // (A tight item's last block ends its line already when it isn't a paragraph.)
          out += `<li>${inner}</li>\n`;
        }
        out += `</${tag}>\n`;
        break;
      }
      case "table": {
        const cell = (tag: string, text: string, i: number) => `<${tag}${b.align[i] ? ` style="text-align: ${b.align[i]}"` : ""}>${inline(inlinesOf(text, d).inlines)}</${tag}>\n`;
        out += `<table>\n<thead>\n<tr>\n${b.head.map((t, i) => cell("th", t, i)).join("")}</tr>\n</thead>\n`;
        out += `<tbody>\n${b.rows.map((r) => `<tr>\n${r.map((t, i) => cell("td", t, i)).join("")}</tr>\n`).join("")}</tbody>\n`;
        out += "</table>\n";
        break;
      }
    }
  }
  return out;
}

export function referenceHtml(markdown: string): string {
  const parsed = blocksOf(markdown);
  return blocks(parsed.blocks, parsed.definitions);
}
