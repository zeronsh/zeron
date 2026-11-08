# Composer text metrics

`ui/ds/composer_metrics.luau` owns the composer's two invisible text nodes
and their measurements. `composer.luau` consumes its result for compact
vs expanded layout and for the expanded text box's height. It no longer
owns `line_w`, `wrapped_h` or their resize callbacks.

This is an adapter around the existing plain-text measurement provider,
not an editor measurement API or a fix for the reported visual glitch.
The visible editor still performs its own shaping/highlighting. Replacing
this provider with editor metrics remains separate work.

## Contract

`Metrics.use { value, width, left, keep? }` returns `nodes` (exactly two
text nodes, placed directly in the existing surface) and `result` (a
computed snapshot):

| Field | Meaning |
| --- | --- |
| `result.revision` | Current requested content/style/width generation |
| `result.width` | Requested expanded content width in logical points |
| `result.status` | `pending`, `stale` or `ready` |
| `result.sample` | Complete last measurement, or nil before any measurement |
| `sample.unwrapped_width` | Natural width used to decide compact vs expanded |
| `sample.wrapped_height` | Natural height at `sample.width`, including a final empty line |
| `sample.line_height` | Resolved composer line-height token for that request |
| `sample.width` | Actual width at which the wrapped text was sampled |
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
new instance and is revalidated against new nodes; retained numbers are
not silently labelled as a measurement performed by the new instance.

## Width and publication

The expanded content width is calculated once at the boundary to the
adapter: available pill width minus both one-point borders and both
16-point expanded text insets. The wrapping node receives that explicit
width. The animated field's changing left/right insets never feed back
into this input. Parent/window width changes still update available space.

Requests change with content, available width and theme typography inputs.
Both resize callbacks and changed requests schedule a coalesced deferred
read of the two nodes. This also updates revisions for equal-size edits,
which do not emit resize notifications. Callbacks for replaced requests
are discarded. One state write publishes both axes and their metadata;
independent width/height state writes cannot mix different requests.

Each new schedule explicitly replaces the old timer. ZGX owns timers
created inside an effect until its next run: relying on a `queued` flag
alone could wait forever after that timer was automatically cancelled.
Cleanup cancels remaining work. No polling timer runs while idle.

Publication is now coalesced, so an animation can begin in a different
frame from the old independent callbacks. Duration/easing and layout rules
are unchanged. The C1 before/after traces have identical destination
rectangles and wrap boundaries for all 786 observed samples; intermediate
animation samples are not claimed to be frame-for-frame identical.

## Validation

```sh
cargo test --locked --manifest-path apps/zgx/Cargo.toml --bin zeron-zgx composer
```

`composer_metrics_tests` covers pending vs empty, stale/replaced requests,
equal-size edits without remounting nodes, target-width independence from
the presented parent, wrapping, trailing newlines, reload and typography
changes. The C1 route-motion tests continue to cover editor identity,
focus, selection and transitions. These are headless checks, not pixel or
macOS presentation validation.
