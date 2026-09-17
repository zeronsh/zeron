These screenshots show the real GPUI shell with isolated sample chats from
`crates/ui/examples/command-palette-fixture.rs`. The fixture uses temporary
storage and does not start an engine or connect to an account.

Run `cargo run -p zeron-ui --example command-palette-fixture` on a desktop, then
press Cmd+K (Ctrl+K on Linux/Windows). Set `ZERON_PALETTE_LIGHT=1` for light mode.

Verified interactively on Linux/X11:

- First-open typing searches the palette, leaving the composer untouched.
- Actions and chat metadata filter together; a chat-only match hides Actions.
- Matching text keeps a compact rounded code wash and code text color while preserving the row font and original text spacing. This applies to action labels and chat metadata.
- The action/history divider spans the card; chat history has no heading.
- Palette chat hover leaves the sidebar pixels unchanged (including Archive state).
- Chat rows use the sidebar height calculation and 2px gap; action rows are 32px.
- Up/Down wraps and scrolls the selected result into view; Enter opens it.
- New chat, New project, and Open settings reach their respective screens.
- New project no longer shows the Cmd+K chip.
- Escape, a second Ctrl+K, and clicking outside dismiss the palette.
- No matches produces the empty state.

The palette uses the composer's frosted tint and 16px backdrop blur, with its
opaque fallback when frost is disabled. The platform-specific Cmd key binding
has not been exercised on macOS in this Linux environment.
