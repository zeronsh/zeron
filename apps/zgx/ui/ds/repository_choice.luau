-- Which repositories a worktree of a whole project checks out: a project
-- folder holding several repositories (and not one itself) gets one
-- worktree of all those chosen, laid out like the project, on one branch.
--
-- A group for the composer's where-it-works panel, under its rows:
--
--   REPOSITORIES                 2 of 3   All
--   [folder] api           ⑂ main          ✓
--   [folder] packages/web  ⑂ develop       ✓
--   [folder] docs          No commits yet      (dimmed: can't be chosen)
--   Each gets a worktree on one new branch, in a folder laid out like
--   the project.
--
-- Rows are the menus' own (washed under the pointer, a check at the end of
-- each chosen one, as a multiple-choice question's options are), and a
-- press anywhere on one turns it on or off; the panel stays open. All /
-- None picks every one that can be chosen, or none. A long list scrolls
-- past `ROWS` rows. While the list is on its way, a line says so.
--
-- `RepositoryChoice { list = fn, loading = fn?, on_toggle = fn(path),
-- on_all = fn(on) }`

local Icon = require("@self/icon")
local FadeText = require("@self/fade_text")
local Menu = require("@self/menu")
local control = require("@self/control")
local locale = require("@self/locale")

export type Row = {
	-- Its path in the project's folder (`packages/web`): its name here, and
	-- where its worktree goes.
	path: string,
	-- The branch checked out there (its worktree starts from it).
	branch: string?,
	chosen: boolean,
	-- Can't be chosen (no commit to branch from).
	disabled: boolean?,
	-- Can't be chosen: no longer where the project includes it from.
	missing: boolean?,
}
export type Props = {
	list: () -> { Row },
	loading: (() -> boolean)?,
	on_toggle: (path: string) -> (),
	on_all: (on: boolean) -> (),
}

-- Rows shown before the list scrolls.
local ROWS = 6
-- A row's height and the gap between rows (the menus' own), as numbers.
local function row_h(): number
	return (ui.theme.get("zeron.menu.row") :: number?) or 28
end
local function gap(): number
	return (ui.theme.get("zeron.list.gap") :: number?) or 2
end

local function RepositoryChoice(props: Props): Node
	local function counts(): (number, number)
		local chosen, can = 0, 0
		for _, r in props.list() do
			if not r.disabled then
				can += 1
				if r.chosen then
					chosen += 1
				end
			end
		end
		return chosen, can
	end
	local function every_one(): boolean
		local chosen, can = counts()
		return can > 0 and chosen == can
	end

	-- All (`on`), or None: a quiet text button at the heading's end.
	local function toggle(id: string, words: Message | string, on: boolean): Node
		return ui.button {
			id = id,
			label = words :: any,
			h = 20,
			px = "$space.1",
			rounded = "$radius.sm",
			items = "center",
			text_size = "$font.size.xs",
			weight = "$font.weight.medium",
			color = "$accent",
			bg = "#00000000",
			hover = { bg = "$hover" },
			active = { bg = "$pressed" },
			focus = control.ring,
			on_click = function()
				props.on_all(on)
			end,
			ui.text { words },
		}
	end

	local heading = ui.row {
		w_full = true,
		h = "$zeron.menu.heading",
		pl = "$space.2",
		pr = "$space.1",
		gap = "$space.2",
		items = "center",
		shrink = 0,
		ui.text {
			locale.t("ds.composer.repositories.heading"),
			text_size = "$font.size.xs",
			weight = "$font.weight.medium",
			color = "$text.tertiary",
			grow = 1,
			aria_hidden = true,
		},
		ui.show(function(): boolean
			local _, can = counts()
			return can > 0
		end, ui.text {
			function(): string
				local chosen, can = counts()
				return locale.s("ds.composer.repositories.chosen", { chosen = chosen, total = can })
			end,
			id = "repositories-count",
			text_size = "$font.size.xs",
			tabular = true,
			color = "$text.tertiary",
		}),
		ui.show(function(): boolean
			local _, can = counts()
			return can > 1 and not every_one()
		end, toggle("repositories-all", locale.t("ds.composer.repositories.all"), true)),
		ui.show(function(): boolean
			local _, can = counts()
			return can > 1 and every_one()
		end, toggle("repositories-none", locale.t("ds.composer.repositories.none"), false)),
	}

	local function row(r: Row, _i: number, live: State<Row>): Node
		local path = r.path
		local function now(): Row
			return live:get()
		end
		local function on(): boolean
			return now().chosen == true
		end
		local function off_limits(): boolean
			return now().disabled == true
		end
		-- (Whether it can be chosen is the row's from the start: a repository
		-- gaining its first commit is listed afresh next time.)
		local fixed_off = r.disabled == true
		local props: ContainerProps = {
			class = "repository-row",
			role = "checkbox",
			label = path,
			checked = on,
			focusable = not fixed_off,
			disabled = fixed_off,
			cursor = if fixed_off then "default" else "pointer",
			hover = if fixed_off then {} else { bg = "$hover" },
			active = if fixed_off then {} else { bg = "$pressed" },
			focus = control.ring,
			on_click = function()
				if not off_limits() then
					props.on_toggle(path)
				end
			end,
			on_key = function(e)
				if (e.key == " " or e.key == "Enter") and not off_limits() then
					props.on_toggle(path)
					return true
				end
				return false
			end,
			Menu.glyph("folder", if r.disabled then "$text.disabled" else nil),
			FadeText {
				path,
				text_size = "$font.size.md",
				color = function(): string
					return if off_limits() then "$text.disabled" else "$text.primary"
				end,
				grow = 1,
			},
			-- Where its worktree starts: its branch, or why it can't.
			ui.row {
				shrink = 0,
				max_w = 120,
				gap = "$space.1",
				items = "center",
				color = "$text.tertiary",
				ui.show(function(): boolean
					return not off_limits() and now().branch ~= nil
				end, Icon { name = "git-branch", size = 12 }),
				FadeText {
					function(): string
						local it = now()
						if it.missing then
							return locale.s("ds.composer.repositories.missing")
						elseif it.disabled then
							return locale.s("ds.composer.repositories.no_commits")
						end
						return it.branch or ""
					end,
					text_size = "$font.size.sm",
					color = function(): string
						return if off_limits() then "$text.disabled" else "$text.tertiary"
					end,
				},
			},
			Menu.check(on),
		}
		Menu.look(props)
		return ui.row(props)
	end

	local rows = ui.each(ui.computed(props.list), row, {
		key = function(r: Row): string
			return r.path
		end,
		w_full = true,
		gap = "$zeron.list.gap",
	})
	local list = ui.scroll {
		id = "repositories-list",
		role = "group",
		label = locale.t("ds.composer.repositories.label"),
		w_full = true,
		overscroll = "contain",
		-- As tall as its rows, up to `ROWS` of them.
		h = function(): number
			local n = math.min(#props.list(), ROWS)
			return math.max(n * (row_h() + gap()) - gap(), 0)
		end :: any,
		rows,
	}
	local loading = props.loading
	return ui.column {
		id = "composer-repositories",
		w_full = true,
		gap = "$zeron.list.gap",
		heading,
		ui.show(function(): boolean
			return #props.list() == 0 and loading ~= nil and loading()
		end, ui.row {
			w_full = true,
			h = "$zeron.menu.row",
			px = "$space.2",
			items = "center",
			ui.text { locale.t("ds.composer.repositories.loading"), text_size = "$font.size.sm", color = "$text.tertiary" },
		}),
		list,
		ui.text {
			locale.t("ds.composer.repositories.hint"),
			px = "$space.2",
			pt = "$space.1",
			pb = "$space.1",
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			color = "$text.tertiary",
			wrap = true,
		},
	}
end

return RepositoryChoice
