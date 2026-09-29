# Plan for the rewrite

The **transcript** is laid out _analytically_: every row's height is known
before it is shown, so scrolling never guesses. Inline `code spans` get chips,
[links](https://zeron.sh/docs) are tappable, and ~~old ideas~~ are struck.

## Steps

1. Parse markdown incrementally (`IncrementalParser::append`).
2. Measure with `rustybuzz` on the bundled Geist faces.
   - Nested bullet with a very/long/path/that/must/wrap/somewhere/in/the/middle/because/it/is/too/wide.rs
   - Another nested item
3. Paint at Rust positions.

- [x] Done task
- [ ] Open task

> Quotes are indented with a bar and muted text. They can hold **bold** and
> multiple lines of prose that wrap naturally.

```rust
fn main() {
    let frame = view.frame();
    for row in frame.rows_in(0.0, 900.0) {
        println!("{} @ {}", row.key, row.y); // a long comment that must scroll horizontally instead of wrapping
    }
}
```

| Crate | Role | Lines |
|:------|:----:|------:|
| zeron-text | measurement + line breaking | 3,100 |
| zeron-markdown | incremental parse | 1,700 |
| zeron-mobile | layout + FFI | 1,400 |

---

CJK: 日本語のテキストも正しく折り返されます。中文也可以。 Emoji: 🚀✨👩‍💻 and https://example.com/a/very/long/url/that/keeps/going/and/going?query=parameters&more=stuff
