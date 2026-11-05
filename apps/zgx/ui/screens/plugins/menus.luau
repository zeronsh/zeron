--!strict
-- What plugins add to Zeron's menus. A menu built when it opens asks for
-- its kind's items and puts them at its end, after a separator:
--
--   local menus = require("@app/screens/plugins/menus")
--   local entries = { …the menu's own… }
--   menus.extend(entries, "session", { id = session_id })
--
-- Kinds: "session" (a session's row; context { id }) and "file" (a file
-- or folder's row in the tree, or the open file; context { path, session? }).
-- A plugin adds an item with its SDK's `zeron.menu { kind, id, label, run }`
-- (it needs "ui.add"); choosing it runs `run(context)` in the plugin.
-- Items show under their plugin's name, so you can tell whose they are.

local M = {}

export type Item = { plugin: string, plugin_name: string, kind: string, id: string, label: string }

-- Published by the host from each plugin's items (src/plugins).
M.items = ui.shared("plugins.menus", {} :: { Item })

-- `kind`'s items as menu entries (ds.ContextMenu's), for `context`.
function M.entries(kind: string, context: { [string]: any }): { any }
	local out: { any } = {}
	local heading: string? = nil
	for _, item in M.items:peek() do
		if item.kind == kind then
			-- Under its plugin's name (the host lists them by plugin).
			if heading ~= item.plugin then
				heading = item.plugin
				table.insert(out, { heading = item.plugin_name })
			end
			table.insert(out, {
				label = item.label,
				on_select = function()
					ui.invoke("plugins.menu", { plugin = item.plugin, id = item.id, context = context })
				end,
			})
		end
	end
	return out
end

-- `entries` with `kind`'s items added at its end, after a separator.
function M.extend(entries: { any }, kind: string, context: { [string]: any }): { any }
	local extra = M.entries(kind, context)
	if #extra > 0 then
		table.insert(entries, { separator = true })
		for _, e in extra do
			table.insert(entries, e)
		end
	end
	return entries
end

return M
