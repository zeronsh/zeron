--!strict
-- Snippets' settings page, drawn by the plugin itself (`zeron.settings_page`).
-- Zeron draws the page's header (the icon, the name, the line under it) and
-- the rows the manifest declares (Message box); this draws what comes after
-- them: the snippets as a list you edit, the one chosen open under it.
--
-- It's the page to copy. Every part is Zeron's own (@ds/settings' Section,
-- Group and Row, and Zeron's fields, buttons and notices), in tokens alone,
-- laid out as Zeron's pages are, its words said as Zeron says them:
--
-- - a list you edit: Settings.Each, a row per snippet, keyed by its id, so a
--   snippet added or renamed changes its own row and no other; "New
--   snippet" at the section title's end; an empty state in the card's place
--   when there are none;
-- - master and detail: choosing a row opens it in the section under the
--   list, built again only as another is chosen (`ui.presence`);
-- - conditional rows: "Try it" and what it shows only for a template, each
--   coming with its own hairline (`ui.show(cond, ui.column { Rule, Row })`);
-- - a validation state: a field's error under it (the name taken, a field
--   that isn't one), and nothing kept while it says one;
-- - an action with a result: Try it, its result drawn in the row under it;
--   Delete asks first, in place;
-- - a declared setting set from here: the "button is hidden" notice turns
--   the declared Show the Snippets button row on (`zeron.setting(id):set`).

local zeron = require("./zeron")("settings-page")
local Settings = require("@ds/settings")
local Button = require("@ds/button")
local TextField = require("@ds/text_field")
local TextArea = require("@ds/text_area")
local Segmented = require("@ds/segmented")
local EmptyState = require("@ds/empty_state")
local Badge = require("@ds/badge").Badge
local Callout = require("@ds/banner").Callout
local snippets = require("./snippets")

-- The snippet open under the list, by id ("": the first). Kept as the page
-- closes and opens again.
local chosen = ui.state("snippets_chosen", "")

-- The snippet open now: the one chosen while it's there, else the first.
local function open_id(): string
	local id = chosen:get()
	if snippets.find(id) then
		return id
	end
	local first = snippets.list()[1]
	return if first then first.id else ""
end

local function new_snippet()
	chosen:set(snippets.add())
end

-- A snippet's text as its row says it: its first line, or that it has none.
local function first_line(text: string): string
	local line = string.match(text, "^%s*([^\n]*)") or ""
	return if line == "" then "Nothing in it yet." else line
end

-- What a field holds, kept as it's typed: a moment after the typing stops,
-- and at once on Enter or leaving it, while `problem` says nothing (a field
-- with an error under it keeps nothing). `put` keeps it.
local function keep_as_typed(draft: State<string>, held: () -> string, problem: () -> string?, put: (text: string) -> ()): () -> ()
	local pending: (() -> ())? = nil
	local function commit()
		if pending then
			pending()
			pending = nil
		end
		if problem() == nil and draft:peek() ~= held() then
			put(draft:peek())
		end
	end
	ui.effect(function()
		draft:get()
		if pending then
			pending()
		end
		pending = ui.after(600, function()
			pending = nil
			commit()
		end)
	end)
	return commit
end

-- A snippet's row in the list: its name, its first line, and, open, a badge
-- saying so. Choosing it opens it under the list.
local function snippet_row(item: snippets.Snippet, _i: number, now: State<any>): Node
	return Settings.Row {
		truncate = true,
		label = function(): string
			return (now:get() :: snippets.Snippet).name
		end,
		description = function(): string
			return first_line((now:get() :: snippets.Snippet).text)
		end,
		badge = ui.show(function(): boolean
			return open_id() == item.id
		end, Badge { "Editing", tone = "neutral" }),
		on_click = function()
			chosen:set(item.id)
		end,
	}
end

-- The snippet open under the list: its name, its kind and its words, a
-- template's Try it, and Delete at the section title's end.
local Editor = ui.component(function(props: { id: string }): Node
	local id = props.id
	local start = snippets.find(id) :: snippets.Snippet
	local name = ui.use_state(start.name)
	local text = ui.use_state(start.text)
	local kind = ui.use_state(start.kind)
	local confirming = ui.use_state(false)
	-- What Try it filled in (nil: not tried since the words last changed).
	local tried = ui.use_state(nil :: string?)

	local function held(): snippets.Snippet
		return snippets.find(id) or start
	end
	local function name_problem(): string?
		return snippets.name_problem(name:get(), id)
	end
	local function text_problem(): string?
		return snippets.text_problem(text:get(), kind:get())
	end
	local save_name = keep_as_typed(name, function(): string
		return held().name
	end, name_problem, function(v: string)
		snippets.update(id, { name = v })
	end)
	keep_as_typed(text, function(): string
		return held().text
	end, text_problem, function(v: string)
		snippets.update(id, { text = v })
	end)
	-- A kind is kept as it's chosen; what was tried goes with any change.
	ui.effect(function()
		local k = kind:get()
		if k ~= held().kind then
			snippets.update(id, { kind = k })
		end
	end)
	ui.effect(function()
		text:get()
		kind:get()
		if tried:peek() ~= nil then
			tried:set(nil)
		end
	end)

	local function is_template(): boolean
		return kind:get() == "template"
	end

	return Settings.Section {
		title = (function(): string
			return held().name
		end) :: any,
		trail = ui.show(function(): boolean
			return not confirming:get()
		end, Button {
			"Delete",
			tone = "ghost",
			size = "sm",
			on_click = function()
				confirming:set(true)
			end,
		}, ui.row {
			gap = "$space.2",
			items = "center",
			Button {
				"Cancel",
				tone = "ghost",
				size = "sm",
				on_click = function()
					confirming:set(false)
				end,
			},
			Button {
				"Delete snippet",
				tone = "danger",
				size = "sm",
				on_click = function()
					local gone = held().name
					chosen:set(snippets.remove(id) or "")
					zeron.toast(`Deleted “{gone}”`)
				end,
			},
		}),
		Settings.Group {
			Settings.Row {
				label = "Name",
				description = "How the message box's Snippets menu lists it.",
				control = TextField {
					value = name,
					name = "Name",
					size = "sm",
					w = 220,
					error = name_problem,
					on_commit = function()
						save_name()
					end,
				},
			},
			Settings.Row {
				label = "Kind",
				description = function(): string
					return if is_template() then "Fills in {date} and {time} as it goes in." else "Goes in as written."
				end,
				control = Segmented {
					value = kind,
					label = "Kind",
					options = { { value = "text", label = "Text" }, { value = "template", label = "Template" } },
				},
			},
			Settings.Row {
				label = "Text",
				description = "What goes in the message box.",
				stacked = true,
				control = TextArea {
					value = text,
					name = "Text",
					rows = 3,
					max_rows = 10,
					w_full = true,
					error = text_problem,
					count = {
						of = function(): number
							return #text:get()
						end,
						soft = snippets.MAX_TEXT - 200,
						max = snippets.MAX_TEXT,
					},
				},
			},
			-- A template's alone: try it, and see what it puts in.
			ui.show(is_template, ui.column {
				w_full = true,
				Settings.Rule(),
				Settings.Row {
					label = "Try it",
					description = function(): string
						if text_problem() ~= nil then
							return "Fix the text first."
						end
						return if tried:get() ~= nil then "What it puts in the message box now:" else "See what it puts in the message box now."
					end,
					control = Button {
						"Try it",
						tone = "secondary",
						size = "sm",
						disabled = function(): boolean
							return text_problem() ~= nil
						end,
						on_click = function()
							tried:set(snippets.fill({ id = id, name = name:peek(), text = text:peek(), kind = kind:peek() }))
						end,
					},
				},
				ui.show(function(): boolean
					return tried:get() ~= nil
				end, ui.column {
					id = "snippets-tried",
					w_full = true,
					px = "$space.4",
					pb = "$space.3",
					ui.text {
						function(): string
							return tried:get() or ""
						end,
						w_full = true,
						wrap = true,
						selectable = true,
						text_size = "$font.size.sm",
						line_height = "$font.line.sm",
						color = "$text.secondary",
					},
				}),
			}),
		},
	}
end)

zeron.settings_page {
	keywords = { "snippets", "templates", "text", "message box" },
	build = function(_page: any): any
		local list = ui.computed(snippets.list)
		local show = zeron.setting("show")
		return {
			Settings.Section {
				title = "Snippets",
				description = "Choose one to change it.",
				trail = Button { "New snippet", tone = "secondary", size = "sm", on_click = new_snippet },
				-- The button they're used from, hidden by the row above: a way
				-- to show it again, from here.
				ui.show(function(): boolean
					return show:get() == false
				end, Callout {
					tone = "info",
					title = "The Snippets button is hidden",
					body = "Your snippets are kept. Show the button to use them from the message box.",
					action = {
						label = "Show it",
						on_click = function()
							show:set(true)
						end,
					},
				}),
				ui.show(function(): boolean
					return #list:get() > 0
				end, ui.column {
					id = "snippets-list",
					w_full = true,
					Settings.Each(list, snippet_row, {
						key = function(s: snippets.Snippet): string
							return s.id
						end,
					}),
				}, Settings.Group {
					ui.column {
						w_full = true,
						py = "$space.6",
						px = "$space.4",
						EmptyState {
							size = "sm",
							icon = "document-add",
							title = "No snippets yet",
							body = "Keep text you use often, and put it in the message box with a click.",
							action = { label = "Add your first snippet", on_click = new_snippet },
						},
					},
				}),
			},
			ui.presence(open_id, function(id: string): Node
				if id == "" then
					return ui.div {}
				end
				return ui.column { id = "snippets-editor", w_full = true, Editor { id = id } }
			end, { w_full = true }),
		}
	end,
}
