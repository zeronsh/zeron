# Message Markdown runtime evidence

Captured on 2026-10-26 on Linux (X11, Mesa software Vulkan) from `message-markdown-fixture`: an offline shell with one chat whose prompts are written in Markdown, one of them sent by another chat through Zeron's `send_message`. The `before-*` captures run the fixture against `main` `962a3207`, from before its last reply gained the fence and quote that `*-blocks.png` shows.

Build and run: `cargo run -p zeron-ui --features appshots-fixture --example message-markdown-fixture` (ZERON_PALETTE_LIGHT=1 for the light theme). The window was captured from the X display with `import`, and the rail tick of the agent-sent prompt hovered with `xdotool`.

- User bubbles render their Markdown like a reply does (strong, emphasis, inline code, lists, fences with highlighting and copy); typed newlines stay line breaks. Prompts that read the same as typed, and prompts carrying file chips, keep the plain bubble.
- The rail's preview card shows `Message from [provider mark] Chat` over an agent-sent prompt instead of its routing header, and renders the prompt and the reply's opening as styled prose clamped to a few lines (`*-hover.png`). Only prose shows: fences, quotes, tables, display formulas and rules are left out (`*-blocks.png`: `Then run:` without its fence, the reply without its output block and quote); a message of nothing but a fence shows its first line.

- `before-dark.png`: SHA256 `626f43fb6a4692f4bab80a5c175e1cc2db746a20c9abcc3e5872eb99e46096ba`
- `after-dark.png`: SHA256 `e815818513b55641511a5848bb1ab85c1b78fc9cd5b545579e6b388edf0024b7`
- `before-light.png`: SHA256 `78461d24621a64fbc5e765e8f73fa32e0246e601a91635ce4b7480a98aa876c8`
- `after-light.png`: SHA256 `770b1568917e173d300779da3731e0eb0bea5fbdd235c9fd9a68da3e5331448a`
- `before-dark-hover.png`: SHA256 `d65c58111f27b3973601d135aefefd93d6451e04c23242ba7ccf4f6d65db8365`
- `after-dark-hover.png`: SHA256 `01fedfe83f754643b8e40fdf72c359ea137bf9d598b05a8c00350c7133b5f6e1`
- `before-light-hover.png`: SHA256 `d2ead849acd730b391f3ac9729c5283cf90cd1be9b46f0349d5cf63c0573e135`
- `after-light-hover.png`: SHA256 `2a29d117f976f649693d636a8076b874ddb5ff2be91af339a78511445242cb2c`
- `after-dark-blocks.png`: SHA256 `e0e89408675e998228337d1326bbc62a99581c987bf7de5566a988800b3ec9e5`
- `after-light-blocks.png`: SHA256 `599b9c5914c27b55534d20dafcc8870f29424bc3e4e8d6138785c73db4c73685`
