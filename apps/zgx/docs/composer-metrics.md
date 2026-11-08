# Composer text metrics

`ui/ds/composer_metrics.luau` adapts the visible editor's natural metrics.
`composer.luau` consumes its result for compact vs expanded layout and the
expanded text box's height. C8 removes both invisible text measurement nodes;
shaping, spans, fallback and IME now come from the editor itself. The framework
API and published revision are described in [composer-editor-api.md](composer-editor-api.md).
This is not a confirmed fix for the reported visual glitch.

## Contract

`Metrics.use { value, width, editor, keep? }` returns `result`, a computed
snapshot. `editor` is the visible textarea ref; the adapter creates no nodes:

| Field | Meaning |
| --- | --- |
| `result.revision` | Current requested content/style/width generation |
| `result.width` | Requested expanded content width in logical points |
| `result.status` | `pending`, `stale` or `ready` |
| `result.sample` | Complete last measurement, or nil before any measurement |
| `sample.unwrapped_width` | Natural width used to decide compact vs expanded |
| `sample.wrapped_height` | Natural height at `sample.width`, including a final empty line |
| `sample.line_height` | Base line height reported by the editor |
| `sample.width` | Requested content width acknowledged by the editor |
| `sample.revision` | Request generation represented by the sample |

`pending` has no sample. `stale` retains a complete old sample while new
content, style or width is being measured. `ready` identifies a sample for
the current request. A zero unwrapped width is valid for a measured empty
draft; it is never used as an initialization sentinel. Unknown/zero
available widths cannot publish a fresh sample.

The consumer deliberately keeps the last sample while stale, preserving
the previous layout until new measures arrive. Before the first sample,
it retains the previous expansion decision and uses the existing height
floor. A newline or explicitly expanded composer still opens immediately.

With `keep`, the complete sample survives a reload. It starts stale in the
new instance and is revalidated against the mounted editor; retained numbers are
not silently labelled as a measurement performed by the new instance.

## Width and publication

The expanded content width is calculated at the adapter boundary: pill
width minus two one-point borders and two 16-point expanded insets. The editor
API measures at that requested width independently of its presented viewport.
The animated field's insets and height never feed back into the measurement.

Requests follow the draft, available width, theme and editor's settled metric
revision (including uncommitted IME). A deferred publication checks both the
request generation and the editor revision. Replacement effects/unmount cancel
old callbacks. Each write publishes both axes and metadata together; no polling
runs while idle. `keep` uses a new `editor_text_metrics` namespace so old
plain-text samples cannot be mistaken for editor measurements after migration.

The layout and transition contracts remain unchanged. Styled text can now
wrap at a different boundary because it is measured with its displayed weights,
rather than approximated by plain text.

## Validation

```sh
cargo test --locked --manifest-path apps/zgx/Cargo.toml --bin zeron-zgx composer
```

`composer_metrics_tests` covers pending vs empty, stale/replaced requests,
equal-size edits without remounting the editor, target-width independence from
the presented parent, wrapping, trailing newlines, reload and typography
changes, formatted content and uncommitted IME, and absence of invisible
measurement nodes. The C1 route-motion tests continue to cover editor identity,
focus, selection and transitions. These are headless checks, not pixel or
macOS presentation validation.
