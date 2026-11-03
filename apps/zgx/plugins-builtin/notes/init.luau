--!strict
-- Notes: a Markdown scratchpad for each main chat, in its panel, synced
-- across your computers (the session doc's `notes`, as the shipping app's
-- Notes tab writes them). Built only from what any plugin can use:
--
--   - A row on the panel's home (`hook:panel.empty`): Notes, N, for a main
--     chat alone; a side chat's panel and a subagent's have none, and N
--     opens nothing there. Which chat that is, Zeron says (`zeron.notes
--     .chat()`: the page's, if it's a main chat).
--   - One tab per chat (`zeron.tab`), opened again rather than twice, the
--     keyboard in it: the editor (doc.luau).
--   - The notes themselves through "notes" (`zeron.notes`), which reaches
--     the chat the plugin is shown in and no other.

local zeron = require("./zeron")("notes")
local PanelHome = require("@ds/panel_home")
local EmptyState = require("@ds/empty_state")
local Doc = require("./doc")(zeron)

local KIND = "notes"

-- Chat `chat`'s notes in `panel`: their one tab (found again by its key),
-- in front, with the keyboard in it.
local function open(panel: any, chat: string)
	Doc.focus(chat)
	panel.open_tab {
		kind = KIND,
		title = zeron.s("tab.title"),
		icon = "notebook",
		data = { key = "notes", chat = chat },
	}
end

zeron.tab(KIND, function(tab: any): Node
	local chat = if type(tab.data) == "table" then tab.data.chat else nil
	if type(chat) ~= "string" or chat == "" then
		return EmptyState { icon = "notebook", size = "sm", fill = true, title = zeron.t("tab.no_chat") :: any }
	end
	return Doc.Tab { chat = chat }
end)

zeron.place {
	at = "hook:panel.empty",
	after = function(): Node
		local session = zeron.context("session")
		local panel = zeron.context("panel")
		if not session or not panel then
			return ui.div {}
		end
		local id = session.id
		-- (Whether this home's chat is the one Zeron opens notes for: a main
		-- chat's, as it's shown.)
		local function mine(): boolean
			return zeron.notes.chat() == id
		end
		-- The home's letters (`panel_key`: `{ key, n }`, bumped as a letter
		-- no row of Zeron's own is pressed while it shows): N is Notes'. A
		-- press is a new `n`; the one there as the row appears was pressed
		-- before it.
		local key = zeron.live("panel_key")
		local now = key()
		local seen = if type(now) == "table" then now.n else nil
		ui.effect(function()
			local k = key()
			if type(k) ~= "table" or k.n == seen then
				return
			end
			seen = k.n
			if string.upper(tostring(k.key or "")) == "N" and mine() then
				-- (After this run: opening a tab writes no state it read.)
				ui.after(0, function()
					open(panel, id)
				end)
			end
		end)
		return ui.show(
			mine,
			PanelHome.Choice {
				id = "notes-open",
				icon = "notebook",
				label = zeron.t("row.label") :: any,
				key = "N",
				on_click = function()
					if mine() then
						open(panel, id)
					end
				end,
			}
		)
	end,
}
