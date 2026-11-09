-- ds/markdown's own drawing of a block, for zgx's markdown view to draw
-- what it doesn't draw itself (ds/prose): code, display math, mermaid,
-- tables and pictures, exactly as ds/markdown draws them. Set as
-- ds/markdown loads (its `block` is its own).

local M = {}

-- `hook(props)`: the view's `block(b, at)` for a text drawn with
-- ds/markdown's props (its veil, find, pictures, links…): block `b`,
-- `at.block` its top-level block's place (from 1), `at.n` its place among
-- that block's parts, `at.find` a find's `{ q, cur, n }` there.
M.hook = (nil :: any) :: (props: any) -> (b: MarkdownBlock, at: any) -> Node

return M
