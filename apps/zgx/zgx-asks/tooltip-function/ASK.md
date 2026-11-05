# Ask: `tooltip` that takes a function or a state

**Priority: low.** A row whose state changes in place (its tooltip naming the state) must rebuild to change the tooltip, or use a node tooltip, which draws in its own popover frame (a double frame the design rejects).

**Today.** `tooltip` takes a string only.

**Ask:** let `tooltip` take a function or a state, as text does.

**Seen on:** zgx `b3371f8` (Zeron's pin), macOS. Zeron: the sidebar row's status corner keeps one `presence` for this.
