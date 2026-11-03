--!strict
-- Git: a chat's changes and history, by Zeron, built only from what any
-- plugin can use. Every piece of git work is done by the plugin's service
-- (service.luau) on the device the chat's checkout is on, with that
-- device's own `git` ("commands": ["git"]): this Mac, or a devbox of yours.
-- Everything it draws is Zeron's design system, lent to plugins (`@ds`).
--
--   - Two tabs in a chat's panel, one each: Changes (the work tree, the
--     branch or the last turn; stage, discard, commit, push) and History
--     (the commits with their branch graph), the branch to switch to at
--     their head (view.luau, changes.luau, history.luau).
--   - Two rows on the panel's empty page (`hook:panel.empty`): Changes and
--     History, each opening its tab (or bringing it forward).
--   - Last turn: as a chat goes to work, its device is told to note where
--     its work tree stands (`turn_start`), so Changes can show what that
--     turn did.
--   - Slots for other plugins (`zeron.slot`): `git.branch` in the Changes
--     head (`git_branch`: the session, its branch and remote, pushed, how
--     far ahead) and `git.commit` in each History row (`git_commit`). The
--     GitHub plugin puts its pull request there; Git itself stays git.

local zeron = require("./zeron")("git")
local PanelHome = require("@ds/panel_home")
local Repo = require("./repo")(zeron)
local words = require("./words")(zeron)
local View = require("./view")(zeron, Repo, words)

-- Changes and History, each a tab of its own; a tab of the one kind
-- they once shared ("git", its view in `data.view`) still opens to its view.
zeron.tab(Repo.KINDS.changes, function(tab: any): Node
	return View { tab = tab, view = "changes", panel = zeron.context("panel") }
end)
zeron.tab(Repo.KINDS.history, function(tab: any): Node
	return View { tab = tab, view = "history", panel = zeron.context("panel") }
end)
-- A commit opened from History, in a tab of its own.
zeron.tab(Repo.KINDS.commit, function(tab: any): Node
	return View { tab = tab, view = "commit", panel = zeron.context("panel") }
end)
zeron.tab("git", function(tab: any): Node
	return View { tab = tab, view = if tab.data and tab.data.view == "history" then "history" else "changes", panel = zeron.context("panel") }
end)

-- The panel's empty page: Changes and History, as its own rows are (a
-- flat row each: its glyph, its name), after them.
zeron.place {
	at = "hook:panel.empty",
	after = function(): Node
		local session = zeron.context("session")
		local panel = zeron.context("panel")
		if not session or not panel then
			return ui.div {}
		end
		-- The home's letters (`panel_key`: `{ key, n }`, bumped as a letter
		-- no row of Zeron's own is pressed while it shows): C and H are
		-- Git's. A press is a new `n`; the one there as the rows appear was
		-- pressed before them.
		local key = zeron.live("panel_key")
		local now = key()
		local seen = if type(now) == "table" then now.n else nil
		ui.effect(function()
			local k = key()
			if type(k) ~= "table" or k.n == seen then
				return
			end
			seen = k.n
			local letter = string.upper(tostring(k.key or ""))
			if letter == "C" or letter == "H" then
				-- (After this run: opening a tab writes no state it read.)
				ui.after(0, function()
					Repo.open_tab(panel, session.id, if letter == "H" then "history" else "changes")
				end)
			end
		end)
		-- (The panel home's own row, lent by the design system, so these
		-- stand exactly as its core rows do: glyph, words, key cap.)
		local function row(id: string, icon: string, label: () -> string, view: string, letter: string): Node
			return PanelHome.Choice {
				id = id,
				icon = icon :: any,
				label = label :: any,
				key = letter,
				on_click = function()
					Repo.open_tab(panel, session.id, view)
				end,
			}
		end
		return ui.column {
			id = "git-empty-rows",
			w_full = true,
			gap = "$zeron.list.gap",
			row("git-open-changes", "file-diff", zeron.t("empty_page.changes"), "changes", "C"),
			row("git-open-history", "git-commit", zeron.t("empty_page.history"), "history", "H"),
		}
	end,
}

-- The panel home's branch row (core's `slot:zeron.panel.branch`): what the
-- branch adds over its base, opening Changes at the Branch scope.
zeron.place {
	at = "slot:zeron.panel.branch",
	after = require("./branch_row")(zeron, Repo),
}

-- Last turn: each chat that goes to work has its device note where its
-- work tree stood (a chat already at work as the window opens is left:
-- its turn began unseen). Every window that sees it says so; the device
-- keeps the first word of a turn.
do
	local was: { [string]: string } = {}
	-- A chat seen for the first time is new if it was made a moment ago
	-- (its first turn starts as it's made); otherwise it was there before
	-- this window, and its turn, if any, began unseen.
	local function new(s: any): boolean
		local made = tonumber(s.created)
		if not made then
			return false
		end
		if made > 1e12 then
			made /= 1000
		end
		return os.time() - made < 60
	end
	ui.effect(function()
		local sessions = zeron.sessions()
		for id, s in sessions do
			local status = s.status or ""
			local before = was[id]
			was[id] = status
			local started = status == "working" and before ~= "working" and (before ~= nil or new(s))
			if started and type(s.cwd) == "string" and s.cwd ~= "" then
				zeron.service(s.device):call("turn_start", { cwd = s.cwd, chat = id }, { timeout = 120 }):after(nil, function(err: any)
					-- (A device without the plugin, or a folder that isn't a
					-- checkout: no turn to note. Said in its log alone.)
					print(`git: turn_start for {id}: {err}`)
				end)
			end
		end
	end)
end
