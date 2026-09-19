# Harness command mapping audit

The composer lists commands supported by each integration. This is not a claim
of parity with every provider's terminal UI. Slash completion is available
throughout the draft. Selected Zeron actions execute immediately and preserve
the draft; provider commands insert references. Provider commands execute only
at the start of a prompt; inline command chips elsewhere remain prompt text.

| Harness | Discovery | Execution |
| --- | --- | --- |
| Claude Code | Initialize control catalog in the selected project directory | Leading slash text through stream-json; ultrathink no longer prefixes and breaks commands |
| Codex | Explicit Zeron catalog: `compact`, `review` | `/compact` → `thread/compact/start`; `/review [instructions]` → `review/start`, inline delivery |
| Cursor | No command discovery in pinned `@cursor/sdk` 1.0.28 | No native slash mapping; text remains an ordinary SDK prompt |
| Devin | ACP session command updates in the selected project | Slash text through `session/prompt` |
| Grok | ACP session command updates in the selected project | Slash text through `session/prompt` |
| Hermes | ACP session command updates in the selected project | Slash text through `session/prompt` |
| Pi | ACP session command updates in the selected project | Slash text through `session/prompt` |
| Antigravity | ACP session command updates in the selected project | Slash text through `session/prompt` |
| OpenCode | Command endpoint scoped to the selected directory; refresh for the live run | Advertised leading command → session command endpoint; v1 `arguments`, v2 `text`; unknown tokens remain prompts |
| Mock | Empty catalog | Test prompt behavior only |

`ListCommands` now resolves chat/space/worktree context like `ListSkills`.
Project-aware discovery bypasses the old global catalog caches. ACP probes open
a session even when initialization already supplied commands, allowing the
session catalog to supersede that fallback.

Codex native operations follow the [app-server protocol](https://developers.openai.com/codex/app-server/).
ACP slash text execution follows the [ACP command protocol](https://agentclientprotocol.com/protocol/v1/slash-commands).
OpenCode uses its [server command API](https://opencode.ai/docs/server/).

## Edge cases requiring review

1. **Terminal UI parity:** Zeron supplies `model`, `new`, `resume`, `settings`,
   `diff`, `files`, `terminal`, `rename`, and `stop` across harnesses; actions
   requiring a conversation appear only in chats. Selecting an action anywhere
   in the draft consumes its trigger and preserves surrounding text and staged
   attachments. Provider name collisions keep both choices, with the Zeron
   action prefixed by `zeron:`. Other terminal-only commands such as Codex
   `/permissions` have no mapping. Unknown slash tokens remain literal,
   including paths; this is not an exhaustive terminal-command registry.
   Cursor has no native command mapping in the pinned SDK.
2. **Review target:** Codex `/review` reviews uncommitted changes; arguments are
   custom instructions. There is no branch/commit target picker yet. Native
   operations queued during a turn run at its boundary; steering during a native
   operation also waits for the boundary.
3. **Compaction:** Codex `/compact` needs an existing conversation, accepts no
   arguments, and fails if that conversation cannot resume. It must not silently
   compact a replacement thread. Both native Codex commands reject attachments.
4. **OpenCode attachments:** Command requests previously discarded attachments.
   They now fail explicitly; supporting attachments requires a protocol-specific
   body mapping. Its detached command HTTP request still relies on session events
   for completion; an HTTP failure without an event is an existing lifecycle gap.
5. **Live catalogs:** ACP updates arriving after the two-second discovery window,
   or after the menu opens, are not propagated live. Same-project definition edits
   may require refreshing the composer context. Discovery can create temporary
   provider sessions. Consider a session-backed catalog subscription.
6. **Skills versus commands:** Agents settings expose independent per-harness
   toggles for `$` completion and separating skills from `/` commands. Both
   default on for Codex and off for the other eight harnesses; users can opt
   Claude Code, OpenCode, or another harness into `$`. Providers can also
   advertise skills inside an undifferentiated command catalog. Those entries
   cannot reliably be filtered without provider metadata. Name collisions remain
   separate typed choices; skill chips preserve their canonical paths.
7. **Provider validation:** Tests use scripted protocol fixtures. Installed
   versions, authentication, interactive terminal-only commands, and OpenCode v2
   command behavior still need live provider smoke testing before release.

## Validation

The September 18 hardening adds canonical-reference delivery matrices for all
nine production harnesses: new and resumed sends, steering, attachments,
startup retry, and editing a queued message before sending it. These use a
recording harness to inspect the engine boundary and persisted transcript;
they do not establish live provider compatibility. Protocol regressions cover
literal code/image contexts, repeated references, punctuation in labels,
native skill identity, and leading command whitespace.

Earlier command integration validation:

- Full harness suite: 250 passed, 10 ignored environment/live-provider tests.
- Harness library: 150 tests passed.
- ACP: 29 passed, including both project catalogs for all four ACP harnesses.
- Claude: 11 passed, 2 ignored live-provider tests.
- Codex: 24 passed, 4 ignored live-provider tests; native operation parameters,
  results, turn-boundary queuing, attachment rejection and resume behavior covered.
- OpenCode: 15 integration tests passed, including command routing and attachment rejection.
- Engine: 181 library tests passed; `cargo check --locked -p zeron` passed.

An existing ACP discovery timeout test failed under parallel suite load and
passed with the suite run serially. The fixtures do not substitute for live
provider QA.
