# Chat mentions

Reference a chat as `@chat:<full-id>` — the full hyphenated id, not a prefix
and not a URL. Zeron renders it as a live link: the reader sees the chat's
title, harness and status, and clicking opens it.

```
Spawned @chat:3f6b2a18-9c4d-4e5f-8a7b-1c2d3e4f5a6b to review the parser.
```

Use mentions wherever you name a chat: in messages to the user, in prompts to
children, in your final summary. A partial id does not link — copy the full id
from the spawn output (the `@chat:<id>` line) or from `zeron chat list`.
