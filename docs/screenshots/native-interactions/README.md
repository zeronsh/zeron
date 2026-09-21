# Native interaction fixture evidence

These captures come from `native-interactions-fixture` at source commit
`085c9917d4845715fd7ad39bd851732d743e95db`. The executable SHA-256 was
`8017af98107df32342c1a866cfffe34e0afe515bee6c4f9b0972c0a59fe3ae9f`.

The fixture was built in the shared target directory with incremental
compilation disabled:

```sh
CARGO_INCREMENTAL=0 cargo build --locked -p zeron-ui --example native-interactions-fixture --features appshots-fixture
```

PID 92855 produced the dark, frosted run. PID 93776 produced the light,
opaque, reduced-motion run. Each run used temporary fixture data, made no
engine IPC connection, read no real-user state, produced ten captures, and
completed every keyboard and synthetic Goal lifecycle assertion.

| Capture | Variant | Logical window | PNG pixels | SHA-256 |
| --- | --- | ---: | ---: | --- |
| `dense-dark-frosted-440x520.png` | Dark, frosted dense trays | 440×520 | 880×1040 | `fcf065cd5922a6f3da3661f61e98bdc1aec9cf17dd1446fefc681fc7ac8bc991` |
| `goal-paused-dark-frosted-840x740.png` | Dark, frosted paused Goal | 840×740 | 1680×1480 | `ea60ab663947010ee38612c64cfe795050271808f44fc0679a04146802ccc2e0` |
| `goal-error-light-opaque-reduced-840x740.png` | Light, opaque, reduced-motion rejected Goal action | 840×740 | 1680×1480 | `080d80df33d36a9ed4b2c048b91d48aef29a18990bfb6eaeb8323b1c686c04d0` |

These static captures show layout and asserted control states. They do not
measure animation smoothness or frame pacing.
