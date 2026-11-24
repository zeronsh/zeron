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
M.ARM_MS = 800

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

-- What a token can publish, every word of it: which of them, each id on its
-- line's own terms, and for all of both kinds what a template can do.
local function scope_lines(scope: any): { () -> string }
	if type(scope) ~= "table" or scope.all == true then
		local both = type(scope) == "table" and type(scope.all_kinds) == "table" and table.find(scope.all_kinds, "template") ~= nil
		if not both then
			return {
				function(): string
					return ui.t("plugins.store.key.scope.all"):get()
				end,
			}
		end
		return {
			function(): string
				return ui.t("plugins.store.key.scope.all_both"):get()
			end,
			function(): string
				return ui.t("plugins.store.key.scope.all_both_hint"):get()
			end,
		}
	end
	return {
		function(): string
			local ids: { string } = {}
			for _, id in (if type(scope.plugins) == "table" then scope.plugins else {}) :: { any } do
				table.insert(ids, plain(id))
			end
			return table.concat(ids, ui.t("plugins.words.list_sep"):get())
		end,
	}
end

-- (How many, first, where there's more than one.)
local function chosen_lines(scope: any): { () -> string }
	local lines = scope_lines(scope)
	local n = if type(scope) == "table" and type(scope.plugins) == "table" then #scope.plugins else 0
	if type(scope) == "table" and scope.all ~= true and n > 1 then
		table.insert(lines, 1, function(): string
			return ui.t("plugins.store.act.chosen", { n = n }):get()
		end)
	end
	return lines
end

-- A fact's words, whole: on as many lines as they take, a word longer than
-- one broken (what's decided here is never cut).
local function whole(lines: { () -> string }): Node
	local texts: { Node } = {}
	for i, words_of in lines do
		table.insert(texts, ui.text {
			words_of,
			w_full = true,
			wrap = true,
			text_size = if i == 1 then "$font.size.md" else "$font.size.sm",
			line_height = if i == 1 then "$font.line.md" else "$font.line.sm",
			color = if i == 1 then "$text.primary" else "$text.secondary",
		})
	end
	return ui.column { grow = 1, basis = 0, min_w = 0, gap = "$zeron.space.hair", table.unpack(texts) }
end

local function facts(it: any): Node
	local items: { any } = {}
	local function add(key: string, lines: { () -> string })
		table.insert(items, { key = ui.t(key), value = whole(lines) })
	end
	local function one(said: () -> string): { () -> string }
		return { said }
	end
	if it.act == "token" then
		add("plugins.store.key.name", one(function(): string
			return plain(it.name)
		end))
		add("plugins.store.key.scope.label", chosen_lines(it.scope))
		add("plugins.store.key.expiry_label", one(function(): string
			return if type(it.expires) == "number" then locale.date(it.expires / 1000) else ui.t("plugins.store.key.expiry.never"):get()
		end))
	else
		add("plugins.store.act.id", one(function(): string
			return plain(it.id)
		end))
		add("plugins.store.act.version", one(function(): string
			return plain(it.version)
		end))
	end
	return ui.div {
		w_full = true,
		shrink = 0,
		ds.Settings.Group {
			ui.column { id = "store-consent-facts", w_full = true, px = "$space.4", py = "$space.3", ds.KeyValue { items = items, key_width = 96 } },
		},
	}
end

local function answer(allow: boolean)
	ui.invoke("store.decide", { allow = allow })
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
					ds.Tile { label = plain(it.name), hue = words.hue(plain(it.id or it.name)), size = 40 },
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
				ui.div { id = "store-consent-why", w_full = true, shrink = 0, ds.Callout { tone = "info", title = ui.t("plugins.store.act.why_title"), body = ui.t("plugins.store.act.why_body") } },
			},
		},
		ui.show(function(): boolean
			return busy() and act == "token"
		end, ui.text {
			id = "store-consent-signing-in",
			ui.t("plugins.store.key.signing_in"),
			w_full = true,
			wrap = true,
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			color = "$text.tertiary",
		}),
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
				ui.t("plugins.store.act.not_now"),
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
