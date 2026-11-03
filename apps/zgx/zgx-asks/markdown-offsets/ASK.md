# Ask: where each `ui.markdown` block starts in its source

**Who needs it:** Zeron's files pane, for review comments on a Markdown preview (the shipping app's `files/markdown_preview.rs`, `comment_block`). A comment is anchored to a line of the file. In the preview, it's left on and shown under the block that holds that line. To do that, the preview needs each top-level block's first source line.

**The problem:** `ui.markdown(text)` returns blocks (`{ t = "p", … }`) with no position. The parser knows it: `markdown/mod.rs` keeps `Top { start, blocks }` per top-level element (the byte offset of its first event) for incremental parsing, but it doesn't reach Luau. Guessing it in Luau, by splitting on blank lines, misplaces comments on lists, fences and tables, where the parser's idea of a block differs from blank-line chunks.

**The ask:** each top-level block carries its source offset, for example:

```lua
for _, b in ui.markdown(text) do
	print(b.at)   -- byte offset (from 0, or 1 as string.find counts) of its first character
end
```

- Top-level blocks only is enough. Nested ones (list items) would be a bonus: production attaches a comment on an inner line to its containing top-level block.
- With `key` (incremental parsing), the offsets of blocks kept from the last parse stay right after an edit above them, or the kept tables are re-made.
- An option (`ui.markdown(text, { offsets = true })`) is fine, if the field costs anything for replies.

**What it unblocks:** preview comments in `ui/screens/files` (the editor's line comments are already wired through `screens/files/comments.luau` and the composer).
