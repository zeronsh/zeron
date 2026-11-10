--!strict
-- Allowing a plugin: who made it, and each thing it asks for in plain
-- words, the risky ones last and in the danger's ink, with what it names
-- exactly as it names it (selectors, screens, programs, its agents' tools).
-- Its window is its own (ui/consent.luau): no plugin and no template runs
-- there, and only that window can allow anything (`plugins.decide`), so
-- nothing standing in for Zeron's screens can say yes on your behalf.
--
-- The host publishes what to show as `consent`:
--   { item = PluginItem?, busy = boolean, error = string?,
--     device_name = string? }   (another of your devices it's on, as that
--                                device lists it: named here, and allowed
--                                on that device alone)
-- (The item's name, version and tools' words come bounded and plain from
-- the host, src/plugins/consent.rs `bounded`.) It answers
-- `plugins.decide { id, allow, hash }` (the hash it was shown: a plugin
-- that changed while you looked asks again) and `plugins.sign { id }`
-- ("It's mine": signs it as yours; this device's key, so not offered for
-- another device's).
--
-- Allow is armed a moment after a sheet shows (`ARM_MS`; the host holds a
-- shorter wait of its own): a window that opens under a click mid-way
-- never takes it. "It's mine" sits with the warning, far from Allow, and
-- asks once more before it signs.

local ds = require("@ds")
local words = require("@screens/plugins/words")

local M = {}

-- How long a sheet shows before Allow takes a click.
M.ARM_MS = 800

local consent = ui.shared("consent", { busy = false } :: any)

local function item(): any
	return consent:get().item
end

local function busy(): boolean
	return consent:get().busy == true
end

-- A version as a version reads (`1.2.0`, `0.3.0-beta.1`), else nothing:
-- never words that could pass for a badge of Zeron's.
local function version(it: any): string?
	local v = it.version
	if type(v) ~= "string" or #v == 0 or #v > 32 or not string.match(v, "^[%w%.%+%-]+$") then
		return nil
	end
	return v
end

-- What it names, as written: each on a line of its own, in the code face.
local function literal(list: { string }?): Node?
	if not list or #list == 0 then
		return nil
	end
	local rows: { Node } = {}
	for _, v in list do
		table.insert(rows, ui.text {
			v,
			font = "$font.mono",
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			color = "$text.secondary",
			wrap = true,
		})
	end
	return ui.column { w_full = true, gap = "$zeron.space.hair", table.unpack(rows) }
end

-- Named things with their own words: a tool's name, and what it tells
-- your agents.
local function named(list: { words.Item }?): Node?
	if not list or #list == 0 then
		return nil
	end
	local rows: { Node } = {}
	for _, t in list do
		table.insert(rows, ui.column {
			w_full = true,
			gap = "$zeron.space.hair",
			ui.text { t.name, font = "$font.mono", text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.primary", wrap = true },
			if t.description
				then ui.text { t.description, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.secondary", wrap = true }
				else nil,
		})
	end
	return ui.column { w_full = true, gap = "$space.2", table.unpack(rows) }
end

-- One ask: its glyph, its line, the line under it, and what it names.
local function line(l: words.Line): Node
	return ui.row {
		w_full = true,
		gap = "$space.3",
		items = "start",
		ui.div {
			pt = "$zeron.space.hair",
			color = if l.risky then "$zeron.tone.danger.ink" else "$text.secondary",
			ds.Icon { name = l.icon :: any, size = 16 },
		},
		ui.column {
			grow = 1,
			min_w = 0,
			gap = "$zeron.space.hair",
			-- (Read as it's drawn: words.Line's words follow the locale.)
			ui.text {
				function(): string
					return l.title
				end,
				text_size = "$font.size.md",
				line_height = "$font.line.md",
				color = if l.risky then "$zeron.tone.danger.ink" else "$text.primary",
				wrap = true,
			},
			if l.detail
				then ui.text {
					function(): string
						return l.detail or ""
					end,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = "$text.secondary",
					wrap = true,
				}
				else nil,
			literal(l.literal),
			named(l.items),
		},
	}
end

-- A section of asks under its heading.
local function section(heading: string | Message, lines: { words.Line }): Node
	local list: { Node } = {}
	for _, l in lines do
		table.insert(list, line(l))
	end
	return ui.column {
		w_full = true,
		gap = "$space.3",
		ui.text { heading, text_size = "$font.size.sm", weight = "$font.weight.medium", color = "$text.secondary" },
		table.unpack(list),
	}
end

-- Who made it, as the signature says.
local function author(it: any): Node
	local kind = it.author and it.author.kind
	if kind == "you" then
		return ds.Badge { ui.t("plugins.consent.made_by_you"), tone = "success", icon = "shield-check" }
	elseif kind == "verified" and it.author.mine == true then
		return ds.Badge { ui.t("plugins.consent.made_on_your_device"), tone = "info", icon = "shield-check" }
	elseif kind == "verified" and type(it.author.username) == "string" and it.author.username ~= "" then
		return ds.Badge { ui.t("plugins.consent.made_by", { username = it.author.username }), tone = "info", icon = "shield-check" }
	end
	return ds.Badge { ui.t("plugins.words.unverified"), tone = "warning", icon = "danger-triangle" }
end

-- (The host's reason as it gives it; else Zeron's words.)
local function why_unknown(it: any): string | Message
	local reason = it.author and it.author.reason
	if it.status == "tampered" then
		return if type(it.source) == "table" then ui.t("plugins.consent.tampered_why") else ui.t("plugins.consent.tampered_why_no_source")
	end
	local where = words.where(it.source)
	return reason or (if where then ui.t("plugins.consent.unverified_why_from", { where = where }) else ui.t("plugins.consent.unverified_why"))
end

-- What it asks, as of this plugin's state: all of it for a new one, what's
-- new for one asking again.
local function asked(it: any): { string }
	if it.ask and #it.ask > 0 then
		return it.ask
	end
	return it.permissions or {}
end

-- One plugin's sheet: what it asks, scrolling, above your answer.
local Sheet = ui.component(function(props: { it: any, elsewhere: string? }): Node
	local it, elsewhere = props.it, props.elsewhere
	-- Allow takes a click once the sheet has shown a moment; "It's mine"
	-- asks once more, in place of the answers.
	local armed = ui.use_state(false)
	local claiming = ui.use_state(false)
	local detailed = ui.use_state(false)
	local details = words.details(it)
	ui.on_mount(function()
		ui.after(M.ARM_MS, function()
			armed:set(true)
		end)
	end)
	local shown = if elsewhere then words.on_device(it, elsewhere) else it
	local lines = words.lines(shown, asked(it))
	local grew = words.diff(shown)
	local again = it.allowed and #it.allowed > 0 and it.status == "pending"
	local unknown = not (it.author and (it.author.kind == "you" or it.author.kind == "verified"))
	local claimable = unknown and it.status ~= "tampered" and not elsewhere
	local v = version(it)

	local asks: Node
	if #lines == 0 and #grew == 0 then
		asks = ui.text { ui.t("plugins.consent.asks_nothing"), text_size = "$font.size.md", color = "$text.secondary", wrap = true }
	elseif #lines == 0 then
		asks = ui.div {}
	else
		asks = section(if again then ui.t("plugins.consent.asks_again") else ui.t("plugins.consent.asks"), lines)
	end

	-- Changed since you allowed it: never allowed as it is now. Removed, or
	-- installed again from where it came from (when that's known), on
	-- this device; another device's is only turned down from here.
	local tampered = it.status == "tampered" and not elsewhere
	local answers = if tampered
		then ui.row {
			w_full = true,
			gap = "$space.2",
			items = "center",
			justify = "end",
			ds.Button {
				id = "consent-remove",
				tone = "secondary",
				size = "md",
				disabled = busy,
				on_click = function()
					ui.invoke("plugins.remove_tampered", { id = it.id, hash = it.hash })
				end,
				ui.t("plugins.consent.remove"),
			},
			if type(it.source) == "table"
				then ds.Button {
					id = "consent-reinstall",
					tone = "primary",
					size = "md",
					disabled = busy,
					on_click = function()
						ui.invoke("plugins.reinstall", { id = it.id, hash = it.hash })
					end,
					ui.t("plugins.consent.reinstall"),
				}
				else nil,
		}
		else ui.row {
		w_full = true,
		gap = "$space.2",
		items = "center",
		justify = "end",
		ds.Button {
			id = "consent-decline",
			tone = "secondary",
			size = "md",
			disabled = busy,
			on_click = function()
				ui.invoke("plugins.decide", { id = it.id, allow = false, hash = it.hash })
			end,
			ui.t("plugins.consent.not_now"),
		},
		-- (Another device's is allowed there, from here: that device's
		-- engine takes it, for that device alone. Never a tampered one.)
		if it.status == "tampered" then nil else ds.Button {
			id = "consent-allow",
			tone = "primary",
			size = "md",
			disabled = function(): boolean
				return busy() or not armed:get()
			end,
			on_click = function()
				if not armed:peek() then
					return
				end
				ui.invoke("plugins.decide", { id = it.id, allow = true, hash = it.hash })
			end,
			if elsewhere then ui.t("plugins.consent.allow_on", { device = elsewhere }) else ui.t("plugins.consent.allow"),
		},
	}

	local confirm_claim = ui.column {
		w_full = true,
		gap = "$space.3",
		ui.text {
			ui.t("plugins.consent.mine_confirm", { name = it.name }),
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			color = "$text.primary",
			wrap = true,
		},
		ui.row {
			w_full = true,
			gap = "$space.2",
			items = "center",
			justify = "end",
			ds.Button {
				id = "consent-mine-cancel",
				tone = "secondary",
				size = "md",
				on_click = function()
					claiming:set(false)
				end,
				ui.t("plugins.consent.mine_cancel"),
			},
			ds.Button {
				id = "consent-mine-sign",
				tone = "secondary",
				size = "md",
				disabled = busy,
				on_click = function()
					claiming:set(false)
					ui.invoke("plugins.sign", { id = it.id })
				end,
				ui.t("plugins.consent.mine_sign"),
			},
		},
	}

	return ui.column {
		w_full = true,
		h_full = true,
		ui.scroll {
			id = "consent-scroll",
			grow = 1,
			min_h = 0,
			w_full = true,
			fade = { 12, 12 },
			fade_overflow = true,
			ui.column {
				w_full = true,
				gap = "$space.5",
				pb = "$space.4",
				ui.row {
					w_full = true,
					gap = "$space.3",
					items = "center",
					words.tile(it, 40),
					ui.column {
						grow = 1,
						min_w = 0,
						gap = "$space.1",
						ui.text {
							ui.t("plugins.consent.allow_title", { name = it.name }),
							heading = 1,
							text_size = "$font.size.lg",
							line_height = "$font.line.lg",
							weight = "$font.weight.semibold",
							color = "$text.primary",
							wrap = true,
						},
						ui.row {
							gap = "$space.2",
							items = "center",
							flex_wrap = true,
							author(it),
							if v then ui.text { v, font = "$font.mono", text_size = "$font.size.sm", color = "$text.secondary" } else nil,
							if elsewhere then ds.Badge { ui.t("plugins.consent.on_device", { device = elsewhere }), icon = "laptop" } else nil,
						},
					},
				},
				if unknown
					then ds.Callout {
						tone = "warning",
						title = if it.status == "tampered" then ui.t("plugins.consent.tampered") else ui.t("plugins.words.unverified"),
						body = why_unknown(it),
					}
					else nil,
				if #grew > 0 then section(ui.t("plugins.consent.grew"), grew) else nil,
				asks,
				-- What of Zeron's interface it reaches, exactly: quiet, folded
				-- away, never an ask of its own.
				if #details > 0
					then ui.column {
						w_full = true,
						gap = "$space.2",
						ui.row {
							ds.Button {
								id = "consent-details",
								tone = "ghost",
								size = "sm",
								on_click = function()
									detailed:toggle()
								end,
								function(): string
									return if detailed:get() then ui.t("plugins.consent.details_hide"):get() else ui.t("plugins.consent.details"):get()
								end,
							},
						},
						ui.show(
							function(): boolean
								return detailed:get()
							end,
							ui.column {
								id = "consent-details-list",
								w_full = true,
								gap = "$space.3",
								table.unpack((function(): { Node }
									local out: { Node } = {}
									for _, d in details do
										table.insert(out, ui.column {
											w_full = true,
											gap = "$zeron.space.hair",
											ui.text { d.title, text_size = "$font.size.sm", color = "$text.secondary" },
											literal(d.list),
										})
									end
									return out
								end)()),
							}
						),
					}
					else nil,
				if elsewhere
					then ui.text {
						ui.t("plugins.consent.allow_there", { device = elsewhere }),
						text_size = "$font.size.sm",
						line_height = "$font.line.sm",
						color = "$text.secondary",
						wrap = true,
					}
					else nil,
				-- (With the warning, never beside Allow.)
				if claimable
					then ui.row {
						ui.show(function(): boolean
							return not claiming:get()
						end, ds.Button {
							id = "consent-mine",
							tone = "ghost",
							size = "sm",
							disabled = busy,
							on_click = function()
								claiming:set(true)
							end,
							ui.t("plugins.consent.mine"),
						}),
					}
					else nil,
			},
		},
		ui.column {
			w_full = true,
			shrink = 0,
			gap = "$space.2",
			pt = "$space.3",
			ui.show(function(): boolean
				return consent:get().error ~= nil
			end, ui.text {
				function(): string
					local error = consent:get().error
					return if type(error) == "string" then words.failure(error) else ""
				end,
				text_size = "$font.size.sm",
				color = "$zeron.tone.danger.ink",
				wrap = true,
			}),
			ui.switch(function(): string
				return if claiming:get() then "claim" else "answer"
			end, { claim = confirm_claim, answer = answers }),
		},
	}
end)

local function body(key: string): Node
	if key == "none" then
		return ds.EmptyState { icon = "check-circle", title = ui.t("plugins.consent.nothing"), size = "sm" }
	end
	-- Another device's: its lines say where it runs.
	local elsewhere = consent:peek().device_name
	if type(elsewhere) ~= "string" or elsewhere == "" then
		elsewhere = nil
	end
	return Sheet { it = item(), elsewhere = elsewhere }
end

function M.View(): Node
	return ui.column {
		id = "consent",
		w_full = true,
		h_full = true,
		p = "$space.6",
		-- (Its window's title bar, with the window's buttons, above.)
		pt = "$space.7",
		-- One plugin's sheet, swapped for the next one's (or none): keyed
		-- by which plugin and what it asks, so it's built again only when
		-- there's something new to read. (The sheet is read once, as a
		-- whole: a decision is made on what was shown.)
		ui.presence(function(): string
			local it = item()
			if not it then
				return "none"
			end
			return `{consent:get().device_name or ""}/{it.id}@{it.hash}:{it.author and it.author.kind or ""}`
		end, body, { w_full = true, h_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
	}
end

return M
