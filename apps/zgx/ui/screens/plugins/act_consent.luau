--!strict
-- What you do as yourself in the store, asked in Zeron's own window
-- (ui/store_consent.luau) once a template that isn't Zeron's has run: the
-- main window is drawn by it, so its buttons aren't proof you pressed them.
-- The host says what's asked, as the engine told it:
--   `store_consent`: { item = { act = "publish" | "unyank" | "token", kind?,
--     id?, name, version?, scope?, expires? }, busy, secret? }
-- and answers `store.decide { allow }`. A new token is made here and shown
-- here (`secret`), once; the main window never has it.

local ds = require("@ds")
local locale = require("@ds/locale")
local words = require("@screens/plugins/words")
local token_block = require("@screens/plugins/token_block")

local M = {}

-- Yes is armed a moment after the window shows, and again each time it comes
-- forward (the host holds one sooner: src/plugins/act_consent.rs `ARMED`), so
-- a click or an Enter meant for the main window never answers it.
M.ARM_MS = require("@screens/plugins/arm").MS

local shown = ui.shared("store_consent", { busy = true } :: any)

local function item(): any
	local now = shown:get()
	return if type(now.item) == "table" then now.item else {}
end

-- What's asked is a thing's own words, drawn as they are (one line of what
-- prints, and short: the host's).
local function plain(v: any): string
	return if type(v) == "string" then v else ""
end

local function title(it: any): string | Message
	if it.act == "publish" then
		return ui.t("plugins.store.act.publish.title", { name = plain(it.name), version = plain(it.version) })
	elseif it.act == "unyank" then
		return ui.t("plugins.store.act.unyank.title", { name = plain(it.name), version = plain(it.version) })
	end
	return ui.t("plugins.store.act.token.title")
end

-- A fact's words, whole: on as many lines as they take, a word longer than
-- one broken (what's decided here is never cut).
local function line(words_of: (() -> string) | Message, quiet: boolean?): Node
	return ui.text {
		words_of :: any,
		w_full = true,
		wrap = true,
		text_size = if quiet then "$font.size.sm" else "$font.size.md",
		line_height = if quiet then "$font.line.sm" else "$font.line.md",
		color = if quiet then "$text.secondary" else "$text.primary",
	}
end

-- One id a line, whole: the code face at a size the longest id there is (40)
-- fits a line in, so none is broken at its hyphens. (It may still wrap: a
-- line that can't is drawn without its end.)
local function id_line(id: string): Node
	return ui.text {
		id,
		class = "store-consent-one-id",
		literal = true,
		w_full = true,
		wrap = true,
		font = "$font.mono",
		text_size = "$font.size.xs",
		line_height = "$font.line.sm",
		color = "$text.primary",
	}
end

-- What a token can publish, every word of it: all of a kind (and for both
-- what a template can do), or which, by how many and then each on its line,
-- templates under their own heading.
local function scope_of(it: any): Node
	local scope = if type(it.scope) == "table" then it.scope else {}
	if scope.all == true or type(scope.plugins) ~= "table" then
		local both = type(scope.all_kinds) == "table" and table.find(scope.all_kinds, "template") ~= nil
		return if both
			then ui.column { grow = 1, basis = 0, min_w = 0, gap = "$zeron.space.hair", line(ui.t("plugins.store.key.scope.all_both")), line(ui.t("plugins.store.key.scope.all_both_hint"), true) }
			else ui.column { grow = 1, basis = 0, min_w = 0, line(ui.t("plugins.store.key.scope.all")) }
	end
	-- (Which are templates, as the host read it of the account: nil, unknown.)
	local known = type(it.templates) == "table"
	local plugins: { string }, templates: { string } = {}, {}
	for _, id in scope.plugins :: { any } do
		if type(id) == "string" then
			table.insert(if known and table.find(it.templates, id) then templates else plugins, id)
		end
	end
	local nodes: { Node } = {}
	local function group(heading: Message, ids: { string })
		if #ids == 0 then
			return
		end
		local lines: { Node } = {}
		for _, id in ids do
			table.insert(lines, id_line(id))
		end
		table.insert(nodes, ui.column { w_full = true, gap = "$space.1", line(heading), ui.column { w_full = true, gap = "$zeron.space.hair", table.unpack(lines) } })
	end
	group(if known then ui.t("plugins.store.count", { n = #plugins }) else ui.t("plugins.store.act.chosen", { n = #plugins }), plugins)
	group(ui.t("templates.store.count", { n = #templates }), templates)
	return ui.column { grow = 1, basis = 0, min_w = 0, gap = "$space.3", table.unpack(nodes) }
end

-- A fact: what it is on its value's first line. (Its room: "Can publish", and
-- the longest id there is beside it.)
local KEY_W = 80
local function row(key: Message, value: Node): Node
	return ui.row {
		w_full = true,
		gap = "$space.3",
		items = "start",
		ui.text { key, w = KEY_W, shrink = 0, text_size = "$font.size.md", line_height = "$font.line.md", color = "$text.tertiary", wrap = true },
		value,
	}
end

local function facts(it: any): Node
	local rows: { Node } = {}
	local function one(words_of: (() -> string)): Node
		return ui.column { grow = 1, basis = 0, min_w = 0, line(words_of) }
	end
	if it.act == "token" then
		table.insert(rows, row(ui.t("plugins.store.key.name"), one(function(): string
			return plain(it.name)
		end)))
		table.insert(rows, row(ui.t("plugins.store.key.scope.label"), scope_of(it)))
		table.insert(rows, row(ui.t("plugins.store.key.expiry_label"), one(function(): string
			return if type(it.expires) == "number" then locale.date(it.expires / 1000) else ui.t("plugins.store.key.expiry.never"):get()
		end)))
	else
		-- (Its name and version are the title's.)
		table.insert(rows, row(ui.t("plugins.store.act.id"), one(function(): string
			return plain(it.id)
		end)))
	end
	return ui.div {
		w_full = true,
		shrink = 0,
		ds.Settings.Group {
			ui.column { id = "store-consent-facts", w_full = true, px = "$space.4", py = "$space.3", gap = "$space.3", table.unpack(rows) },
		},
	}
end

local function answer(allow: boolean)
	ui.invoke("store.decide", { allow = allow })
end

-- The flow's status line: its room kept in every one of a token's steps, so
-- nothing moves as it comes and goes.
local function status(waiting: (() -> boolean)?): Node
	return ui.row {
		id = "store-consent-status",
		w_full = true,
		min_h = ui.theme.get("font.line.sm") :: number?,
		items = "center",
		if waiting
			then ui.show(waiting, ui.text {
				id = "store-consent-signing-in",
				ui.t("plugins.store.key.signing_in"),
				grow = 1,
				min_w = 0,
				wrap = true,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = "$text.tertiary",
			})
			else nil,
	}
end

local Asking = ui.component(function(_props: {}): Node
	local it = item()
	local act = if it.act == "publish" or it.act == "unyank" then it.act else "token"
	local function busy(): boolean
		return shown:get().busy == true
	end
	local armed = ui.use_state(false)
	local arming = 0
	local raised = ui.computed(function(): number
		local n = shown:get().raised
		return if type(n) == "number" then n else 0
	end)
	ui.effect(function()
		local _ = raised:get()
		arming += 1
		local mine = arming
		armed:set(false)
		ui.after(M.ARM_MS, function()
			if mine == arming then
				armed:set(true)
			end
		end)
	end)
	return ui.column {
		w_full = true,
		h_full = true,
		gap = "$space.5",
		-- What's asked, all of it: it keeps its height and scrolls where the
		-- window can't hold it (a name on many lines, many ids), never cut;
		-- the answer stays under it.
		ui.scroll {
			id = "store-consent-scroll",
			w_full = true,
			grow = 1,
			basis = 0,
			min_h = 0,
			fade = { 12, 12 },
			fade_overflow = true,
			ui.column {
				id = "store-consent-said",
				w_full = true,
				shrink = 0,
				gap = "$space.5",
				ui.row {
					w_full = true,
					gap = "$space.3",
					items = "center",
					-- (A token is no plugin: the key, on the neutral tile.)
					if act == "token"
						then ds.Tile { size = 40, inside = ds.Icon { name = "key-minimalistic", size = 18 } }
						else ds.Tile { label = plain(it.name), hue = words.hue(plain(it.id or it.name)), size = 40 },
					ui.text {
						title(it),
						heading = 1,
						grow = 1,
						min_w = 0,
						text_size = "$font.size.lg",
						line_height = "$font.line.lg",
						weight = "$font.weight.semibold",
						color = "$text.primary",
						wrap = true,
					},
				},
				ui.text {
					ui.t(`plugins.store.act.{act}.body`),
					w_full = true,
					wrap = true,
					text_size = "$font.size.md",
					line_height = 20,
					color = "$text.secondary",
				},
				facts(it),
				-- Why it's asked here: said, quietly, after what's asked.
				ui.column {
					id = "store-consent-why",
					w_full = true,
					shrink = 0,
					gap = "$zeron.space.hair",
					ui.text { ui.t("plugins.store.act.why_title"), w_full = true, wrap = true, text_size = "$font.size.sm", line_height = "$font.line.sm", weight = "$font.weight.medium", color = "$text.primary" },
					ui.text { ui.t("plugins.store.act.why_body"), w_full = true, wrap = true, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.secondary" },
				},
			},
		},
		if act == "token" then status(busy) else nil,
		ui.row {
			w_full = true,
			gap = "$space.2",
			items = "center",
			ui.div { grow = 1 },
			ds.Button {
				id = "store-consent-decline",
				tone = "secondary",
				size = "md",
				on_click = function()
					answer(false)
				end,
				-- (While it waits on the sign-in: what it does then.)
				function(): string
					return if busy() and act == "token" then ui.t("plugins.store.key.stop_waiting"):get() else ui.t("plugins.store.act.not_now"):get()
				end,
			},
			ds.Button {
				id = "store-consent-allow",
				tone = "primary",
				size = "md",
				busy = busy,
				disabled = function(): boolean
					return busy() or not armed:get()
				end,
				on_click = function()
					if armed:peek() and not shown:peek().busy then
						answer(true)
					end
				end,
				ui.t(`plugins.store.act.{act}.allow`),
			},
		},
	}
end)

-- The new token, until it's put away.
local function made(): Node
	local copied = ui.use_state(false)
	return ui.column {
		id = "store-consent-token",
		w_full = true,
		h_full = true,
		gap = "$space.5",
		ui.text {
			ui.t("plugins.store.key.secret_title"),
			heading = 1,
			w_full = true,
			wrap = true,
			text_size = "$font.size.lg",
			line_height = "$font.line.lg",
			weight = "$font.weight.semibold",
			color = "$text.primary",
		},
		ui.text {
			ui.t("plugins.store.key.secret_body"),
			w_full = true,
			wrap = true,
			text_size = "$font.size.md",
			line_height = 20,
			color = "$text.secondary",
		},
		token_block.Block {
			secret = function(): string?
				local s = shown:get().secret
				return if type(s) == "string" and s ~= "" then s else nil
			end,
			copy = function(token: string): any
				return ui.call("secret.copy", { text = token })
			end,
			copied = copied,
		},
		ui.div { grow = 1 },
		status(nil),
		ui.row {
			w_full = true,
			items = "center",
			ui.div { grow = 1 },
			ds.Button {
				id = "store-consent-done",
				tone = "primary",
				size = "md",
				on_click = function()
					ui.invoke("close", nil)
				end,
				ui.t("plugins.store.key.done"),
			},
		},
	}
end

function M.View(): Node
	return ui.column {
		id = "store-consent",
		w_full = true,
		h_full = true,
		p = "$space.6",
		-- (Its window's title bar, with the window's buttons, above.)
		pt = "$space.7",
		ui.presence(function(): string
			local now = shown:get()
			if type(now.secret) == "string" and now.secret ~= "" then
				return "made"
			end
			-- (What's asked, whole: a sheet is of one thing from start to end.)
			local it = now.item
			return if type(it) == "table" then `ask:{it.act}:{plain(it.id)}:{plain(it.version)}:{plain(it.name)}` else "none"
		end, function(key: string): Node
			return if key == "made" then made() elseif key == "none" then ui.div {} else Asking {}
		end, { w_full = true, h_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
	}
end

return M
