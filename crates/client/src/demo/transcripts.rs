//! Demo transcripts: hand-written fixtures that exercise every renderer path
//! (headings, nested/ordered/task lists, tables, quotes, links, inline code,
//! code blocks in several languages, long paths/URLs, CJK + emoji, tool
//! groups, questions, errors, attachments), the synthetic big transcripts
//! for benchmarks, and the scripted streaming reply.

use zeron_doc::{
    MessagePart, MessageRole, MessageStatus, SessionMessageEntry, SubagentStatus, ToolDiffStat,
};
use zeron_proto::{TodoItem, ToolCall, UserInputQuestion};

pub(crate) const PHONE: &str = "ios-demo";

pub(crate) fn text(id: &str, body: &str) -> MessagePart {
    MessagePart::Text {
        id: id.into(),
        text: body.into(),
    }
}

fn reasoning(id: &str, body: &str) -> MessagePart {
    MessagePart::Reasoning {
        id: id.into(),
        text: body.into(),
    }
}

pub(crate) fn tool(id: &str, call: ToolCall, is_error: bool, output: Option<&str>) -> MessagePart {
    MessagePart::Tool {
        id: id.into(),
        call,
        is_error,
        resolved: true,
        output: output.map(str::to_owned),
        diff: None,
        output_ref: None,
        output_bytes: output.map(|o| o.len() as u64),
        diff_ref: None,
        diff_stats: None,
        subagent_ref: None,
        subagent_status: None,
        subagent_tail: None,
    }
}

fn edit(id: &str, path: &str, additions: u64, deletions: u64) -> MessagePart {
    let mut part = tool(
        id,
        ToolCall::EditFile {
            path: path.into(),
            old_string: None,
            new_string: None,
        },
        false,
        None,
    );
    if let MessagePart::Tool { diff_stats, .. } = &mut part {
        *diff_stats = Some(vec![ToolDiffStat {
            path: path.into(),
            additions,
            deletions,
        }]);
    }
    part
}

fn exec(command: &str) -> ToolCall {
    ToolCall::Exec {
        command: command.into(),
    }
}

fn read(path: &str) -> ToolCall {
    ToolCall::ReadFile { path: path.into() }
}

pub(crate) fn entry(
    id: &str,
    role: MessageRole,
    device: &str,
    created_at: i64,
    parts: Vec<MessagePart>,
) -> SessionMessageEntry {
    SessionMessageEntry {
        id: id.into(),
        role,
        parts,
        created_at,
        device_id: device.into(),
        status: Some(MessageStatus::Complete),
        continuation_of: None,
        duration_ms: (role == MessageRole::Assistant).then_some(48_000),
    }
}

fn user(id: &str, at: i64, body: &str) -> SessionMessageEntry {
    entry(id, MessageRole::User, PHONE, at, vec![text("t0", body)])
}

fn assistant(id: &str, host: &str, at: i64, parts: Vec<MessagePart>) -> SessionMessageEntry {
    entry(id, MessageRole::Assistant, host, at, parts)
}

/// Paths the demo serves generated images for.
pub(crate) const DEMO_IMAGES: &[(&str, u32, u32, u32)] = &[
    ("/Users/dev/.zeron/uploads/veil-before.png", 960, 540, 1),
    ("/Users/dev/.zeron/uploads/scroll-tall.png", 400, 800, 2),
    ("/Users/dev/.zeron/uploads/square.png", 600, 600, 3),
];

const VEIL_PLAN: &str = r#"## Veil port plan

The desktop veil (`veil.rs`) multiplies a fading alpha into each appended chunk's text color — **paint-layer only**, so shaping and wrapping never change. Three invariants to carry over:

1. Chunk spans keep their *exact* byte length when split
2. Fade duration tracks the append cadence: `clamp(ema × 3, 120, 400)` ms
3. Re-attach seeds the baseline — only post-switch appends animate
   - a cold open renders history fully opaque
   - a warm reopen keeps the in-flight fade

| Constant | Value | Where |
| --- | ---: | --- |
| `VEIL_MIN_FADE_MS` | 120 | `crates/ui/src/markdown/veil.rs` |
| `VEIL_MAX_FADE_MS` | 400 | same |
| `VEIL_CURVE_POW` | 1.6 | same |

> The curve is `1 − (1−p)^1.6` — fast attack, soft landing.
> It reads calm even at 140ms token cadence."#;

const VEIL_IMPL: &str = r#"Implementation lands in `Veil.swift`:

```swift
func veilOpacity(_ p: Double) -> Double {
    1 - pow(1 - p, 1.6)  // fast attack, soft landing
}

// Duration follows the streaming cadence EMA.
let duration = min(max(ema * 3, 120), 400)
```

And the Rust side stays the source of truth:

```rust
pub fn veil_alpha(elapsed_ms: f32, duration_ms: f32) -> f32 {
    let p = (elapsed_ms / duration_ms).clamp(0.0, 1.0);
    1.0 - (1.0 - p).powf(VEIL_CURVE_POW)
}
```

The row keeps one `RowVeil` while streaming and drops it on the live→complete flip, exactly like the desktop lifecycle. Details in [the veil design note](https://github.com/zeron-sh/zeron/blob/main/docs/design/transcript-veil.md#lifecycle)."#;

const CJK_REPLY: &str = r#"### Grapheme-safe fading

Chunk boundaries now snap to **extended grapheme clusters**, so these all fade as single units:

- 日本語のテキストは文字単位でフェードします。
- 中文段落也一样：每个汉字都是一个独立的字素簇。
- 한국어 음절도 마찬가지로 처리됩니다.
- Emoji sequences: 👩🏽‍💻 🏳️‍🌈 👨‍👩‍👧‍👦 🇯🇵 — each one cluster, never split mid-ZWJ.
- Mixed runs: `veil` + 絵文字 🎉 + العربية stay aligned.

| Script | Sample | Clusters |
| --- | --- | ---: |
| Japanese | こんにちは世界 | 7 |
| Emoji ZWJ | 👨‍👩‍👧‍👦 | 1 |
| Flags | 🇨🇦🇯🇵 | 2 |

The splitter lives in `/Users/dev/zeron/crates/ui/src/markdown/veil/grapheme_boundaries_for_streamed_chunks.rs` and is covered by a table test:

```python
CASES = [
    ("日本語", 3),
    ("👨‍👩‍👧‍👦", 1),
    ("é", 1),  # combining acute accent
]
for text, expected in CASES:
    assert len(grapheme_clusters(text)) == expected, text
```

```ts
export function clusters(text: string): string[] {
  const seg = new Intl.Segmenter(undefined, { granularity: "grapheme" });
  return Array.from(seg.segment(text), (s) => s.segment);
}
```

Reference: https://www.unicode.org/reports/tr29/#Grapheme_Cluster_Boundaries_and_Extended_Grapheme_Clusters_in_Full_Detail"#;

const VEIL_LIVE_PREFIX: &str = "All 14 veil tests pass. Pushing `veil-fade` and";

/// The remainder `chat-veil`'s live entry streams when the session opens.
pub(crate) const VEIL_LIVE_REST: &str = r#" opening the pull request against `main`:

- [x] `cargo test -p zeron-ui veil` — 14 passed
- [x] `xcodebuild -scheme Zeron build`
- [ ] Screenshots for the PR description

```bash
git push -u origin veil-fade
gh pr create --base main --title "Stream pull request status on every client"
```

PR **#90** is open: https://github.com/zeron-sh/zeron/pull/90"#;

fn veil(host: &str, now: i64) -> Vec<SessionMessageEntry> {
    let attach = crate::attachments::with_attachments(
        "Port the streaming fade-in veil from the desktop transcript. It must never affect layout — opacity only, split at chunk boundaries. Here's how it looks today:",
        &[DEMO_IMAGES[0].0.to_owned()],
    );
    let mut live = assistant(
        "m6",
        host,
        now - 60_000,
        vec![
            text("t0", "Opening the PR now. Running the checks first:"),
            tool(
                "k1",
                exec("cargo test -p zeron-ui veil -- --nocapture"),
                false,
                Some("test result: ok. 14 passed; 0 failed; 0 ignored"),
            ),
            text("t1", VEIL_LIVE_PREFIX),
        ],
    );
    live.status = Some(MessageStatus::Streaming);
    live.duration_ms = None;
    vec![
        user("m1", now - 3_500_000, &attach),
        assistant(
            "m2",
            host,
            now - 3_400_000,
            vec![
                reasoning(
                    "r0",
                    "The veil must be paint-only. Check how veil.rs splits chunk spans and where the fade duration comes from before touching the Swift side.",
                ),
                text("t0", VEIL_PLAN),
                tool("k1", read("crates/ui/src/markdown/veil.rs"), false, None),
                tool(
                    "k2",
                    ToolCall::Search {
                        pattern: "VEIL_".into(),
                        path: Some("crates/ui".into()),
                    },
                    false,
                    Some("crates/ui/src/markdown/veil.rs:12: pub const VEIL_MIN_FADE_MS"),
                ),
                edit("k3", "apps/ios/Zeron/Transcript/Veil.swift", 84, 12),
                tool(
                    "k4",
                    exec(
                        "xcodebuild -scheme Zeron -destination 'platform=iOS Simulator,name=iPhone 17 Pro' build",
                    ),
                    false,
                    Some("** BUILD SUCCEEDED **"),
                ),
                text("t1", VEIL_IMPL),
            ],
        ),
        user(
            "m3",
            now - 1_800_000,
            "Also check it handles CJK and emoji — 日本語のテキストと絵文字 🎉 must fade per grapheme, not per byte.",
        ),
        assistant(
            "m4",
            host,
            now - 1_700_000,
            vec![
                tool(
                    "k1",
                    ToolCall::Glob {
                        pattern: "crates/ui/src/markdown/**/*.rs".into(),
                    },
                    false,
                    None,
                ),
                tool(
                    "k2",
                    ToolCall::WebFetch {
                        url: "https://www.unicode.org/reports/tr29/#Grapheme_Cluster_Boundaries"
                            .into(),
                        prompt: None,
                    },
                    false,
                    None,
                ),
                tool(
                    "k3",
                    ToolCall::Todo {
                        items: vec![
                            TodoItem {
                                text: "Snap chunk splits to grapheme clusters".into(),
                                done: true,
                            },
                            TodoItem {
                                text: "Table test for ZWJ sequences".into(),
                                done: true,
                            },
                            TodoItem {
                                text: "Measure veil cost on 600-turn transcript".into(),
                                done: false,
                            },
                        ],
                    },
                    false,
                    None,
                ),
                edit("k4", "crates/ui/src/markdown/veil.rs", 31, 4),
                text("t0", CJK_REPLY),
            ],
        ),
        user("m5", now - 120_000, "Ship it and open the PR."),
        live,
    ]
}

fn picker(host: &str, now: i64) -> Vec<SessionMessageEntry> {
    vec![
        user(
            "m1",
            now - 400_000,
            "The model picker shows stale catalogs after switching devices — where should the catalog come from?",
        ),
        assistant(
            "m2",
            host,
            now - 380_000,
            vec![
                tool(
                    "k1",
                    ToolCall::Search {
                        pattern: "list_models".into(),
                        path: None,
                    },
                    false,
                    None,
                ),
                tool("k2", read("crates/ui/src/shell/pickers.rs"), false, None),
                text(
                    "t0",
                    "Two viable sources — the local device's harness install, or the space's owning device. The desktop recently moved to the latter (`aa128a6`). Before I wire the RPC, two decisions:",
                ),
                MessagePart::Input {
                    id: "req-1".into(),
                    request_id: "req-1".into(),
                    questions: vec![
                        UserInputQuestion {
                            id: "q1".into(),
                            header: "Catalog source".into(),
                            question:
                                "Which device should serve harness/model catalogs for the picker?"
                                    .into(),
                            options: vec![
                                "Space's device (Recommended)".into(),
                                "Local device".into(),
                                "Union of both".into(),
                            ],
                            multi_select: false,
                        },
                        UserInputQuestion {
                            id: "q2".into(),
                            header: "Harnesses".into(),
                            question: "Which harnesses should the picker offer on phones?".into(),
                            options: vec![
                                "Claude Code".into(),
                                "Codex".into(),
                                "OpenCode".into(),
                                "Grok".into(),
                            ],
                            multi_select: true,
                        },
                    ],
                    resolved: false,
                },
            ],
        ),
    ]
}

fn tabs(host: &str, now: i64) -> Vec<SessionMessageEntry> {
    vec![
        user(
            "m1",
            now - 1_000_000,
            "Tool group headers turn red when any child fails — they should stay quiet, chips carry the error.",
        ),
        assistant(
            "m2",
            host,
            now - 950_000,
            vec![
                tool(
                    "k1",
                    ToolCall::Search {
                        pattern: "group_header_color".into(),
                        path: None,
                    },
                    false,
                    None,
                ),
                tool(
                    "k2",
                    exec("cargo test -p zeron-ui tool_group"),
                    true,
                    Some("error[E0425]: cannot find value `danger_muted` in this scope"),
                ),
                edit("k3", "crates/ui/src/shell/transcript.rs", 6, 9),
                tool(
                    "k4",
                    exec("cargo test -p zeron-ui tool_group"),
                    false,
                    Some("test result: ok. 9 passed"),
                ),
                text(
                    "t0",
                    "Done — the header keeps `text_muted` even on failure; only the chip label and the summary segment (\"1 failed\") pick up `danger`. Matches the desktop fix in `1749890`.",
                ),
            ],
        ),
    ]
}

fn errored(host: &str, now: i64) -> Vec<SessionMessageEntry> {
    let mut failed = assistant(
        "m2",
        host,
        now - 2_000_000,
        vec![
            text(
                "t0",
                "Reproducing the flush-timer wake on a hibernated Durable Object:",
            ),
            tool(
                "k1",
                exec("npx wrangler dev --test-scheduled"),
                true,
                Some("✘ [ERROR] Could not resolve \"node:async_hooks\""),
            ),
            MessagePart::Error {
                id: "e0".into(),
                message: "Harness exited with status 1: rate limited — retry in 2m".into(),
            },
        ],
    );
    failed.status = Some(MessageStatus::Aborted);
    vec![
        user(
            "m1",
            now - 2_100_000,
            "Why does the flush timer wake hibernated rooms every 5s?",
        ),
        failed,
    ]
}

fn scroll(host: &str, now: i64) -> Vec<SessionMessageEntry> {
    let attach = crate::attachments::with_attachments(
        "The transcript goes blank until I scroll after reopening a long chat.",
        &[DEMO_IMAGES[1].0.to_owned()],
    );
    vec![
        user("m1", now - 5_000_000, &attach),
        assistant(
            "m2",
            host,
            now - 4_900_000,
            vec![
                text(
                    "t0",
                    r#"## Root cause

"Blank until scroll" means the content offset is stranded **past the end** of the content after a geometry change:

1. The keyboard dismisses → the viewport grows
2. Rows below re-measure shorter
3. The offset still points at the old bottom

### Fix

Clamp the invariant on *every* geometry change instead of adding per-trigger clamps:

```swift
override func layoutSubviews() {
    super.layoutSubviews()
    let maxY = max(0, contentSize.height - bounds.height + adjustedContentInset.bottom)
    if contentOffset.y > maxY { contentOffset.y = maxY }
}
```

- See `TranscriptScrollMatrixTests` for the 36-case matrix
- [UIScrollView docs](https://developer.apple.com/documentation/uikit/uiscrollview)"#,
                ),
                tool(
                    "k1",
                    exec("xcodebuild test -only-testing:ZeronTests/TranscriptScrollMatrixTests"),
                    false,
                    Some("Executed 36 tests, with 0 failures"),
                ),
            ],
        ),
    ]
}

fn home(host: &str, now: i64) -> Vec<SessionMessageEntry> {
    vec![
        user(
            "m1",
            now - 7_200_000,
            "Clean up ~/Downloads — anything older than 90 days goes to an archive folder.",
        ),
        assistant(
            "m2",
            host,
            now - 7_100_000,
            vec![
                tool(
                    "k1",
                    exec("find ~/Downloads -maxdepth 1 -mtime +90 | wc -l"),
                    false,
                    Some("412"),
                ),
                text(
                    "t0",
                    r#"Found **412** items older than 90 days. Plan:

- [x] Create `~/Downloads/_archive-2026`
- [x] Move files, keep folder structure
  - `.dmg` installers → deleted instead (re-downloadable)
  - screenshots → `~/Pictures/Screenshots/old`
- [ ] Empty the Trash (waiting for your OK)

```bash
mkdir -p ~/Downloads/_archive-2026
find ~/Downloads -maxdepth 1 -mtime +90 ! -name '_archive-*' \
  -exec mv {} ~/Downloads/_archive-2026/ \;
```

```json
{ "moved": 377, "deleted": 35, "freedBytes": 18432000000 }
```"#,
                ),
            ],
        ),
    ]
}

fn cjk(host: &str, now: i64) -> Vec<SessionMessageEntry> {
    vec![
        user(
            "m1",
            now - 600_000,
            "テキストのレイアウトを多言語で確認して 🌏 — 中文、한국어、emoji も。",
        ),
        assistant(
            "m2",
            host,
            now - 590_000,
            vec![text(
                "t0",
                r#"# 多言語レイアウト確認 🌏

日本語の長い段落です。禁則処理により、句読点（、。）や閉じ括弧」は行頭に来ません。長い文章でも自然に折り返されることを確認してください。これはテスト用の文章です。

中文段落：排版引擎需要正确处理标点挤压和避头尾规则，例如“引号”和（括号）不应出现在行首。

한국어 문단: 단어 단위 줄바꿈이 올바르게 동작하는지 확인합니다.

> 引用ブロック内の日本語 — 👩🏽‍💻 と 🏳️‍🌈 は一つの書記素クラスタです。

1. 第一項目
2. 第二項目 `inline コード`
3. 第三項目 [リンク](https://ja.wikipedia.org/wiki/禁則処理)

| 言語 | 例 |
| --- | --- |
| 日本語 | 吾輩は猫である |
| 中文 | 我思故我在 |
| 한국어 | 안녕하세요 |
| emoji | 🎉🚀✨🧪 |"#,
            )],
        ),
    ]
}

fn short(host: &str, now: i64, prompt: &str, reply: &str) -> Vec<SessionMessageEntry> {
    vec![
        user("m1", now - 120_000, prompt),
        assistant("m2", host, now - 100_000, vec![text("t0", reply)]),
    ]
}

/// Fixture transcript for a demo chat (`None` = starts empty).
pub(crate) fn fixture(chat_id: &str, host: &str, last_activity: i64) -> Vec<SessionMessageEntry> {
    let now = last_activity;
    match chat_id {
        "chat-veil" => veil(host, crate::now_ms()),
        "chat-picker" => picker(host, now),
        "chat-tabs" => tabs(host, now),
        "chat-errored" => errored(host, now),
        "chat-ios-scroll" => scroll(host, now),
        "chat-home" => home(host, now),
        "chat-cjk" => cjk(host, now),
        "chat-fanout" => fanout(chat_id, host, crate::now_ms()),
        "chat-background" => background(chat_id, host, crate::now_ms()),
        "chat-deploy" => short(
            host,
            now,
            "Audit the wrangler config for hibernation hygiene.",
            "Flush timer now only arms while dirty; ping/pong uses the auto-response path so the DO never wakes for keepalives.",
        ),
        "chat-ios-keyboard" => short(
            host,
            now,
            "The composer jumps when the keyboard animates in.",
            "Tracking `keyboardLayoutGuide` instead of the notification frame fixes it — the composer now rides the guide with the system curve.",
        ),
        "chat-blog" => short(
            host,
            now,
            "Draft the launch post outline.",
            "## Outline\n\n1. Why a phone viewport\n2. The CRDT under the hood\n3. What's next",
        ),
        "chat-side" => short(
            host,
            now,
            "What EMA window does the veil use?",
            "α = 0.2 over inter-append gaps, seeded from the first two chunks.",
        ),
        "chat-oklch" => short(
            host,
            now,
            "OKLCH conversion drifts from the desktop.",
            "Gamma encode matches now.",
        ),
        "chat-presence" => short(
            host,
            now,
            "Presence beats spam the DO.",
            "Batched to one beat per 25s.",
        ),
        _ => Vec::new(),
    }
}

const PROSE: &str = r#"The dropdown's open handler awaits `loadRefs()` **before** it paints, so
the menu can't render until the full ref index resolves. On a repo with
many refs that's a visible hang, and it is paid again on every open
because the result is never memoized between mounts.

Three things stack up here:

1. `loadRefs()` walks every ref and builds a fresh array each call
2. The handler `await`s it inline instead of rendering an empty menu
3. `useRefIndex` has no cache, so remount re-does the whole walk

| Stage | Cost | Cached |
| --- | --- | --- |
| `loadRefs` | O(refs) | no |
| `useRefIndex` | O(refs) | no |
| paint | O(visible) | n/a |

> The fix is to paint first and fill in — the index can arrive late.

```ts
const refs = useRefIndex()          // memoized, suspense-free
useEffect(() => { void warmRefIndex() }, [])
return <Menu items={refs ?? []} loading={refs == null} />
```"#;

/// `turns` synthetic user/assistant pairs (legacy BenchRunner shape).
pub(crate) fn synthetic(turns: u32, host: &str, start: i64) -> Vec<SessionMessageEntry> {
    let mut out = Vec::with_capacity(turns as usize * 2);
    for i in 0..turns as i64 {
        let at = start + i * 60_000;
        out.push(user(
            &format!("u{i}"),
            at,
            &format!("Turn {i}: the ref dropdown still hangs on open — dig into it."),
        ));
        let mut parts = vec![text(
            "t0",
            &format!("## Pass {i}: where the dropdown stalls\n\n{PROSE}"),
        )];
        for t in 0..4i64 {
            let index = i * 4 + t;
            let call = match index % 4 {
                0 => exec("rg -n 'refDropdown' src/components --glob '!*.test.ts'"),
                1 => read("src/components/refs/RefDropdown.tsx"),
                2 => ToolCall::EditFile {
                    path: "src/components/refs/useRefIndex.ts".into(),
                    old_string: None,
                    new_string: None,
                },
                _ => ToolCall::Search {
                    pattern: "loadRefs\\(".into(),
                    path: None,
                },
            };
            parts.push(tool(&format!("k{i}.{t}"), call, index % 17 == 0, None));
        }
        parts.push(text(
            "t1",
            &format!("Landed the pass-{i} change behind `refIndexCache`. Open latency drops to\na paint, and the index warms in the background on first hover."),
        ));
        out.push(assistant(&format!("a{i}"), host, at + 1_000, parts));
    }
    out
}

/// One step of a scripted assistant turn.
pub(crate) enum Step {
    Reasoning(String),
    Text(String),
    Tool {
        call: ToolCall,
        output: Option<String>,
        is_error: bool,
        run_ms: u64,
    },
    Question(Vec<UserInputQuestion>),
}

const REPLY: &str = r#"Here's how the streamed reply renders on this device:

- Markdown re-parses **only the tail** — the last two top-level blocks
- New text fades in through the paint-only veil
- The transcript stays glued to the bottom until you scroll up

```rust
// The desktop constant carries over verbatim.
const STREAM_COMMIT_MS: u64 = 120;
```"#;

const REPLY_TAIL: &str = r#"| Stage | Per token |
| --- | --- |
| doc decode | one entry |
| row rebuild | one row |

When the turn settles, this entry flips `streaming → complete`, the veil drops, and the row ids stay stable so nothing flickers."#;

/// The scripted reply to a prompt (the legacy demo responder, plus a thinking
/// block and a tool group so every live-row kind streams).
pub(crate) fn reply(prompt: &str, long: bool) -> Vec<Step> {
    let first_line = prompt.lines().next().unwrap_or("").trim();
    let quoted: String = first_line.chars().take(80).collect();
    let body = if long {
        [REPLY; 12].join("\n\n---\n\n")
    } else {
        REPLY.to_owned()
    };
    vec![
        Step::Reasoning(format!(
            "The user asked: \"{quoted}\". Walk through the streaming path end to end and show a small code sample."
        )),
        Step::Text(body),
        Step::Tool {
            call: read("crates/doc/src/transcript_delta.rs"),
            output: None,
            is_error: false,
            run_ms: 350,
        },
        Step::Tool {
            call: exec("cargo test -p zeron-client transcript"),
            output: Some("test result: ok. 12 passed; 0 failed".into()),
            is_error: false,
            run_ms: 900,
        },
        Step::Text(REPLY_TAIL.to_owned()),
    ]
}

/// Reply after answering a question panel.
pub(crate) fn answered(labels: &[String]) -> Vec<Step> {
    let picked = if labels.is_empty() {
        "your defaults".to_owned()
    } else {
        labels.join(", ")
    };
    vec![Step::Text(format!(
        "Going with **{picked}**. Wiring `ListModels` to the session's host device now — the picker will refresh whenever the host changes."
    ))]
}

/// A reply that ends by asking a question (prompts containing "?ask").
pub(crate) fn asking() -> Vec<Step> {
    vec![
        Step::Text("Before I continue I need one decision:".into()),
        Step::Question(vec![UserInputQuestion {
            id: "q1".into(),
            header: "Scope".into(),
            question: "Should the fix cover Android too?".into(),
            options: vec!["Yes, both platforms".into(), "iOS only".into()],
            multi_select: false,
        }]),
    ]
}

// ── subagents ─────────────────────────────────────────────────────────────

/// One demo subagent: its spawn id, task, agent type, and how it ended.
struct DemoSpawn {
    id: &'static str,
    task: &'static str,
    agent_type: &'static str,
    status: SubagentStatus,
    report: Option<&'static str>,
}

const fn done(
    id: &'static str,
    task: &'static str,
    agent_type: &'static str,
    report: &'static str,
) -> DemoSpawn {
    DemoSpawn {
        id,
        task,
        agent_type,
        status: SubagentStatus::Done,
        report: Some(report),
    }
}

const fn failed(
    id: &'static str,
    task: &'static str,
    agent_type: &'static str,
    report: &'static str,
) -> DemoSpawn {
    DemoSpawn {
        id,
        task,
        agent_type,
        status: SubagentStatus::Failed,
        report: Some(report),
    }
}

const fn running(id: &'static str, task: &'static str, agent_type: &'static str) -> DemoSpawn {
    DemoSpawn {
        id,
        task,
        agent_type,
        status: SubagentStatus::Running,
        report: None,
    }
}

/// "chat-fanout": three turns of spawns — 13 completed (the list pages),
/// 2 failed, 3 still running under a working parent.
const FANOUT: &[(i64, &[DemoSpawn])] = &[
    (
        40 * 60_000,
        &[
            done(
                "fo-registry",
                "Map registry row writers",
                "Explore",
                "Four writers: engine sessions, workspace host, MCP create_chat and the demo host. All go through `upsert_session`.",
            ),
            done(
                "fo-chat2",
                "Trace chat2 room joins",
                "Explore",
                "Viewers join `chat2/{id}` once the row reads roomGen ≥ 2; hosts claim on first join.",
            ),
            done(
                "fo-outbox",
                "Audit the outbox retry loop",
                "general-purpose",
                "Retries back off to 30s and never give up; a 409 re-bases the cursor.",
            ),
            done(
                "fo-cursor",
                "Check cursor persistence",
                "Explore",
                "Cursor and snapshot commit in one transaction; the cursor is sampled before export.",
            ),
            done(
                "fo-presence",
                "Measure presence beat cost",
                "general-purpose",
                "One beat per 25s per device; the DO coalesces bursts.",
            ),
            done(
                "fo-blob",
                "Review sidecar blob uploads",
                "Explore",
                "Fire-and-forget PUTs, 4 KiB-capped outputs; a lost upload degrades to the summary.",
            ),
            done(
                "fo-seen",
                "Trace the seen marker",
                "Explore",
                "`lastSeenAt` is written on open and on every new turn while the chat is visible.",
            ),
            done(
                "fo-evict",
                "Audit warm-session eviction",
                "general-purpose",
                "Detached, quiet sessions past the cap are dropped oldest first.",
            ),
            failed(
                "fo-edge",
                "Load-test the edge relay",
                "general-purpose",
                "wrangler dev exited with status 1: port 8787 already in use",
            ),
        ],
    ),
    (
        20 * 60_000,
        &[
            running("fo-soak", "Soak-test reconnect storms", "general-purpose"),
            done(
                "fo-schema",
                "Diff the session doc schema",
                "Explore",
                "No breaking changes since epoch 2; `subagentTail` stays optional.",
            ),
            done(
                "fo-heartbeat",
                "Check heartbeat throttling",
                "Explore",
                "Status rows refresh at most every 10s while a run streams.",
            ),
            done(
                "fo-queue",
                "Walk the queue lease flow",
                "general-purpose",
                "Leases expire after 60s and demand review before delivery.",
            ),
            done(
                "fo-pins",
                "Verify pin ordering",
                "Explore",
                "Fractional keys between neighbours; ties break on chat id.",
            ),
            done(
                "fo-archive",
                "Check archive round trip",
                "Explore",
                "Archive keeps the row and its pins; unarchive restores both.",
            ),
            failed(
                "fo-ios",
                "Run the iOS snapshot tests",
                "general-purpose",
                "xcodebuild: no simulator matching 'iPhone 17' is installed",
            ),
        ],
    ),
    (
        3 * 60_000,
        &[
            running("fo-fuzz", "Fuzz the row merge", "general-purpose"),
            running("fo-docs", "Draft the sync design notes", "Plan"),
        ],
    ),
];

/// "chat-background": the parent's turn is over; two subagents still run.
const BACKGROUND: &[(i64, &[DemoSpawn])] = &[(
    12 * 60_000,
    &[
        running("bg-e2e", "Run the full e2e suite", "general-purpose"),
        running("bg-bench", "Benchmark transcript layout", "general-purpose"),
        done(
            "bg-lint",
            "Lint the workspace",
            "general-purpose",
            "clippy is clean: 0 warnings across 42 crates.",
        ),
    ],
)];

fn spawns_of(chat_id: &str) -> &'static [(i64, &'static [DemoSpawn])] {
    match chat_id {
        "chat-fanout" => FANOUT,
        "chat-background" => BACKGROUND,
        _ => &[],
    }
}

/// Subagents a demo chat reports as running on its session row.
pub(crate) fn running_subagents(chat_id: &str) -> u32 {
    spawns_of(chat_id)
        .iter()
        .flat_map(|(_, spawns)| spawns.iter())
        .filter(|s| s.status == SubagentStatus::Running)
        .count() as u32
}

fn spawn_part(chat_id: &str, spawn: &DemoSpawn) -> MessagePart {
    let mut part = tool(
        spawn.id,
        ToolCall::Unknown {
            name: format!("Agent: {}", spawn.task),
            input: Some(serde_json::json!({ "subagent_type": spawn.agent_type })),
        },
        false,
        spawn.report,
    );
    if let MessagePart::Tool {
        subagent_ref,
        subagent_status,
        ..
    } = &mut part
    {
        *subagent_ref = Some(format!("{chat_id}--sub--{}", spawn.id));
        *subagent_status = Some(spawn.status);
    }
    part
}

fn fanout(chat_id: &str, host: &str, now: i64) -> Vec<SessionMessageEntry> {
    let mut entries = vec![user(
        "m1",
        now - 42 * 60_000,
        "Audit every sync path before the release. Fan out — one subagent per area — and report back.",
    )];
    for (turn, (ago, spawns)) in spawns_of(chat_id).iter().enumerate() {
        let mut parts: Vec<MessagePart> = spawns.iter().map(|s| spawn_part(chat_id, s)).collect();
        parts.insert(
            0,
            text(
                "t0",
                match turn {
                    0 => "Splitting this across the stack — one agent per area:",
                    1 => "First wave is back. Following up on what it surfaced:",
                    _ => "Two more for the edges, then I'll write it up:",
                },
            ),
        );
        let mut entry = assistant(&format!("a{turn}"), host, now - ago, parts);
        entry.duration_ms = Some(60_000);
        entries.push(entry);
    }
    entries
}

fn background(chat_id: &str, host: &str, now: i64) -> Vec<SessionMessageEntry> {
    let (ago, spawns) = spawns_of(chat_id)[0];
    let mut parts: Vec<MessagePart> = spawns.iter().map(|s| spawn_part(chat_id, s)).collect();
    parts.push(text(
        "t1",
        "Lint is clean. The e2e suite and the layout benchmark keep running in the background — I'll pick their results up when they land.",
    ));
    vec![
        user(
            "m1",
            now - ago - 30_000,
            "Run lint, the e2e suite and the layout benchmark in the background.",
        ),
        assistant("a0", host, now - ago, parts),
    ]
}

/// The transcript of one demo subagent (`{chat}--sub--{spawn}`), shaped the
/// way the engine writes a subagent doc: the task, then the agent's turn.
/// A running one is still streaming.
pub(crate) fn subagent(
    chat_id: &str,
    doc_id: &str,
    host: &str,
) -> Option<Vec<SessionMessageEntry>> {
    let now = crate::now_ms();
    let (ago, spawn) = spawns_of(chat_id).iter().find_map(|(ago, spawns)| {
        spawns
            .iter()
            .find(|s| doc_id == format!("{chat_id}--sub--{}", s.id))
            .map(|s| (*ago, s))
    })?;
    let started = now - ago;
    let mut entries = vec![entry(
        "u0",
        MessageRole::User,
        host,
        started,
        vec![text("t0", spawn.task)],
    )];
    let mut parts = vec![
        tool(
            "s1",
            ToolCall::Search {
                pattern: "sync".into(),
                path: Some("crates".into()),
            },
            false,
            Some("crates/sync/src/chat_client.rs\ncrates/client/src/live/room.rs"),
        ),
        tool("s2", read("crates/client/src/live/room.rs"), false, None),
    ];
    let mut turn = entry(
        "a0",
        MessageRole::Assistant,
        host,
        started + 2_000,
        Vec::new(),
    );
    match spawn.status {
        SubagentStatus::Running => {
            parts.push(tool(
                "s3",
                exec("cargo test -p zeron-sync -- --nocapture"),
                false,
                None,
            ));
            if let MessagePart::Tool { resolved, .. } = &mut parts[2] {
                *resolved = false;
            }
            parts.push(text("t1", "Still going — the soak loop is on its third reconnect storm and nothing has diverged yet."));
            turn.status = Some(MessageStatus::Streaming);
            turn.duration_ms = None;
        }
        SubagentStatus::Done => {
            parts.push(text("t1", spawn.report.unwrap_or("Done.")));
            turn.duration_ms = Some(95_000);
        }
        SubagentStatus::Failed => {
            parts.push(tool("s3", exec("./scripts/check.sh"), true, spawn.report));
            parts.push(text(
                "t1",
                &format!(
                    "I couldn't finish: {}",
                    spawn.report.unwrap_or("the run failed.")
                ),
            ));
            turn.duration_ms = Some(41_000);
        }
    }
    turn.parts = parts;
    entries.push(turn);
    Some(entries)
}
