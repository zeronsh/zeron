-- The Control room's Scratchpad as the host keeps it (src/scratchpad.rs):
-- one document per account, Markdown and a drawing, and its history
-- (`WatchScratchpad`, whole on every change). Nothing written here is work.
-- account/orchestration_sample.luau's `scratchpad` is a whole one.
--
--   scratchpad.now:get()   { loaded, text, rev, updated_at?, updated_by,
--                            device_id?, shapes, history }
--                          `updated_by`, a revision's `by`: "person",
--                          "brain_dump" or "other"; history newest first,
--                          its first the document as it is now
--   scratchpad.set_text(text)      a promise of the new rev
--   scratchpad.set_shapes(shapes)  a promise of the new rev
--   scratchpad.restore(id)         a snapshot put back: a promise of the rev
--   scratchpad.revision(id)        a promise of one snapshot, whole
--                                  { id, at, edited_at, by, text, shapes,
--                                    restored_from? }
--
-- A shape: { id, kind ("stroke", "rect", "ellipse", "arrow", "text", or a
-- newer renderer's), x, y, w, h, points = { { x, y } }, text, color (a
-- token: "fg", "muted", "accent", "red", "orange", "yellow", "green",
-- "blue", "purple"), stroke?, z }, in the drawing's own units.

local M = {}

M.now = ui.shared("scratchpad", {
	loaded = false,
	text = "",
	rev = "",
	updated_by = "person",
	shapes = {},
	history = {},
} :: any)

function M.set_text(text: string): any
	return ui.call("scratchpad.set_text", text)
end

function M.set_shapes(shapes: { any }): any
	return ui.call("scratchpad.set_shapes", shapes)
end

function M.restore(id: string): any
	return ui.call("scratchpad.restore", id)
end

function M.revision(id: string): any
	return ui.call("scratchpad.revision", id)
end

return M
