# Ask: pictures in Markdown that aren't alone in their paragraph

**Who needs it:** Zeron's reply pictures (`apps/zgx/ui/ds/markdown.luau`, `ds/reply_picture.luau`). Agents write pictures inside paragraphs all the time: "Here it is:" with the picture on the next line, or two or three pictures on consecutive lines. `ui.markdown` drops all but a lone picture.

Seen on zgx `aaa304eb`. `zgx test apps/zgx/zgx-asks/markdown-pictures` runs the repro (3 failing tests).

## What `ui.markdown` does today (crates/zgx-script/src/markdown/mod.rs, `TagEnd::Image`)

A picture is a block (`{ t = "img", src, alt }`) only when the paragraph's runs are empty. Anywhere else, its alt text is pushed as plain text, and the `src` is lost:

| Text | Blocks today | Wanted |
|---|---|---|
| `Here it is:\n![shot](a.png)` | `p` ("Here it is: shot") | `p`, `img` |
| `![a](a.png)\n![b](b.png)` | `img`, `p` (" b") | `img`, `img` |
| `![a](a.png)\n\nAfter.` | `img`, `p` (empty), `p` | `img`, `p` |

The empty paragraph in the last row comes from `TagEnd::Paragraph` pushing the paragraph after the picture took its place.

## Asks

- (a) A picture on a line of its own in a paragraph (only whitespace beside it) splits the paragraph there: the words before it, an `img` block, the words after.
- (b) A picture inline among words keeps its `src`: a run `{ x = alt, img = src }`, so an app can draw it or link it rather than lose it.
- (c) No empty `p` is left where a picture took its paragraph.

## Zeron's workaround meanwhile

`ds/markdown_pictures.luau`: before parsing, a line that is one picture is given a blank line on each side (outside code fences). After parsing, pictures that follow one another, past the empty paragraphs, are merged into one `pics` block. `ds/markdown_find` parses the same way, so find's block numbers still agree. An inline picture among words still shows only its alt text.
