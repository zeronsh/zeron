# Ask: a selectable node that selects as one unit

**Who needs it:** Zeron's display math (`ui/ds/math.luau`). A formula is drawn on a `ui.canvas`; its TeX rides as unseen text laid under it, so a selection across the reply washes the formula's box and copy gives `$$tex$$`. The same goes for any drawn thing whose copy is text (a diagram's source, a chart's data).

**The problem:** the unseen text is ordinary text, so a drag that starts or ends inside the formula selects part of the TeX, and copy gives `$$\frac{-b \pm \sqrt{b` — half a formula. Zeron's shipping app treats a formula as one character: in a selection or not, whole.

**The ask:** a way to make a node select as one unit, with the text it copies as. For example:

```lua
ui.canvas { …, selectable_as = "$$\\frac{a}{b}$$" }   -- one unit in a `selectable` parent
```

- A drag selects it whole once it touches it, or not at all. Its box washes in the selection colour behind its content.
- Copy gives `selectable_as` in its place among the text around it.
- Accessibility reads `selectable_as` (or its `label`) as one item.

**Today's workaround** (`M.Display` in `ui/ds/math.luau`): the unseen text is mono, sized a little wider than the formula and clipped to its box, so a whole selection fills it. A partial one is still possible.
