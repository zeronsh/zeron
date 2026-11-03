--!strict
-- GitHub: your pull requests in Zeron, as a plugin built only from what any
-- plugin can use. GitHub is asked through its CLI as you're signed in to it
-- on this Mac (`zeron.run`, "commands": ["gh"]), what it says kept in the
-- plugin's own database (`zeron.db`).
-- Everything it draws is Zeron's design system, lent to plugins (`@ds`), so
-- it follows the template in use.
--
-- Step 1: each session's pull request beside its title in the sidebar, the
-- design system's pull request badge (badge.luau); its state and title in
-- the session's hover card (card_row.luau), not a tooltip; a click opens
-- its session and the pull request in that chat's panel.
--
-- Step 2: that pull request in the chat's panel (`github-pr` tabs), the
-- gallery's review: Summary, Code, Activity (viewer.luau).
--
-- Step 3: the pull request board, a page of its own (board.luau). And
-- notifications as your pull requests move (notices.luau).

local zeron = require("./zeron")("github")
local Badge = require("./badge")
local CardRow = require("./card_row")
-- GitHub through gh, as you're signed in on this Mac (pulls.luau), and each
-- session's pull request, kept up to date (watch.luau).
local pulls = require("./pulls")(zeron)
local watch = require("./watch")(zeron, pulls)
local Viewer = require("./viewer")(zeron, pulls)
local Board = require("./board")(zeron, pulls, Viewer)
-- Merged and closed pull requests, said (notices.luau).
require("./notices")(zeron, watch)

zeron.place {
	at = ".session-title",
	after = function(): Node
		local id = zeron.need("session").id
		local pr = watch.pr(id)
		-- The row's session's panel (read here: a click comes later).
		local panel = zeron.context("panel")
		return ui.show(function(): boolean
			return pr() ~= nil
		end, Badge {
			words = zeron,
			pr = pr,
			on_open = function()
				local now = pr()
				if now and panel then
					-- Its session, then the pull request in its panel.
					zeron.open({ session = id })
					panel.open_tab {
						kind = "github-pr",
						-- (Named by what it is: its title, its number while it has none.)
						title = if now.title and now.title ~= "" then now.title else `#{now.number}`,
						icon = "pull-request",
						data = { url = now.url, number = now.number, session = id },
					}
				end
			end,
		})
	end,
}

-- The same pull request in the session's hover card, a row under its
-- branch (Zeron's card), where the badge's state and title are said: the
-- badge has no tooltip of its own.
zeron.place {
	at = ".session-card-end",
	after = function(): Node
		local pr = watch.pr(zeron.need("session").id)
		return ui.show(function(): boolean
			return pr() ~= nil
		end, CardRow(pr))
	end,
}

-- In the Git plugin's Changes head (its `git.branch` slot): this branch's
-- pull request, or Create pull request (git_slot.luau).
local GitSlot = require("./git_slot")(zeron, watch)
zeron.place {
	at = "slot:git.branch",
	after = GitSlot.build,
}

-- In the Git plugin's History: faces, pull requests, its page on GitHub
-- (git_history.luau).
require("./git_history")(zeron, pulls)

-- In the panel home's details (core's `zeron.panel.details` slot): the
-- session's pull request, under its branch (panel_row.luau).
zeron.place {
	at = "slot:zeron.panel.details",
	after = require("./panel_row")(zeron, watch),
}

zeron.tab("github-pr", function(tab: any): Node
	return Viewer { tab = tab }
end)

-- Step 3: the pull request board, a page of its own in the sidebar.
zeron.page {
	id = "pulls",
	-- (A function: the sidebar's entry follows the language.)
	title = zeron.t("page.pulls"),
	icon = "pull-request",
	build = function(page: any): Node
		-- (Props of its own: what a place gives is read-only. Its panel: the
		-- page's own, where a pull request opens as a tab.)
		return Board { page = page, panel = zeron.context("panel") }
	end,
}
