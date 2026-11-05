-- The chat page: a new chat (`Home`), a session (`Session(id)`) and the
-- window's one composer (`Composer`), which travels between them. The
-- shell's router shows `Home` or `Session` in its page pane, and places
-- `Composer()` once over that pane, outside what it routes, so the draft,
-- the keys and the spring between the two places carry across a move.
--
--   local chat = require("@app/screens/chat")
--   chat.Home()
--   chat.Session(id)    id: a string, a function or a state; nil, the
--                       route's session
--   chat.Composer()
--
-- Each part is required as `@app/screens/chat/…`, so a template or a
-- trusted plugin can replace one whole, and they reload live as they're
-- saved. What they show lives outside them (the host's transcript and
-- session, screens/chat/shared's draft and scroll), so a part made again
-- comes back where it was, mid-reply included. A part that fails to load
-- shows a quiet line in its place; the others go on.

-- A part, or a stand-in saying it couldn't be shown (the error goes to
-- the log, for whoever is editing it).
-- (`what`: its name's catalog key, said in the locale shown.)
local function part(path: string, what: string): any
	local ok, loaded = pcall(function()
		return (require :: any)(path)
	end)
	if ok and loaded ~= nil then
		return loaded
	end
	print(`{path} didn't load: {loaded}`)
	return function(): Node
		return ui.column {
			w_full = true,
			h_full = true,
			items = "center",
			justify = "center",
			ui.text { ui.t("chat.init.couldnt_show", { what = function(): string
				return ui.t(what):get()
			end }), literal = true, color = "$text.tertiary", text_size = "$font.size.sm" },
		}
	end
end

return {
	Home = part("@app/screens/chat/home", "chat.init.home"),
	Session = part("@app/screens/chat/session", "chat.init.session"),
	Composer = part("@app/screens/chat/composer", "chat.init.composer"),
	-- The chat's data and calls (screens/chat/store), and the composer's
	-- shared draft (screens/chat/shared), for the shell's commands (⌘L to
	-- the composer: `shared.focus:add(1)`).
	store = require("@app/screens/chat/store"),
	shared = require("@app/screens/chat/shared"),
	-- The composer's model picker opened (the shell's ⌘/): true if there
	-- was a composer to open it in.
	open_model_picker = function(): boolean
		return require("@app/screens/chat/shared").open_model_picker()
	end,
}
