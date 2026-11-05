-- Settings → Providers → Add an agent: every agent that speaks the Agent
-- Client Protocol (the ACP registry, account/registry), a page of its own
-- inside Providers rather than a panel over it (the gallery's
-- demos/example_add_agent). The registry searched; one agent read before
-- it is added (who makes it, its version, license and source, what adding
-- it takes on the device, whether its download is checked, where its
-- models and sign-in come from, and what it may do once there); the add in
-- the same place, done or failed; and, once added, its version, an update
-- when the registry has a newer one, and removing it.
--
-- The rows are a keyed `ui.each` whose rows get their agent as state: a
-- row is built once and changes in place as it is added or updated, and a
-- search adds and takes rows away without building the rest again.
--
--   add_agent.registry_page(nav)   the registry
--   add_agent.agent_page(id, nav)  one agent
--
-- `nav`: { go = function(view) (views: "agents", "add", "agent:<id>"),
-- device = function(): string (the device's name, in the locale shown) }.

local ds = require("@ds")
local host = require("@app/screens/settings/host")
local registry = require("@account/registry")

local M = {}

export type Nav = { go: (view: string) -> (), device: () -> string }

-- The way back from a page under Providers: the page it came from, a quiet
-- link led by a chevron over the title.
local function back(label: Message, to: string, nav: Nav): Node
	return ui.row {
		self = "start",
		gap = "$space.1",
		items = "center",
		cursor = "pointer",
		role = "button",
		focusable = true,
		label = ui.t("settings.providers.add.back", {
			page = function(): string
				return (label :: any):get()
			end,
		}),
		color = "$text.tertiary",
		hover = { color = "$text.primary" },
		on_click = function()
			nav.go(to)
		end,
		ds.Icon { name = "alt-arrow-left", size = 14 },
		ui.text { label, text_size = "$font.size.sm", line_height = "$font.line.sm" },
	}
end

-- A page under Providers: where it stands, over its title, at the page's
-- own measure.
local function sub_page(where: Node, page: Node): Node
	return ui.column {
		w_full = true,
		-- (On its parent's middle, whatever wraps it: as every Settings page.)
		self = "center",
		-- (A theme number, read as it is built: max_w takes points.)
		max_w = ui.theme.get("zeron.settings.measure") :: number?,
		gap = "$space.4",
		where,
		page,
	}
end

-- The marks of the agents Zeron drives itself, by their harness (the
-- registry lists them too: their rows wear Zeron's mark, as their cards do).
local BUILT_IN_MARKS: { [string]: string } = {
	["claude-code"] = "claude",
	codex = "openai",
	cursor = "cursor",
	devin = "devin",
	grok = "grok",
	opencode = "opencode",
	pi = "pi",
	antigravity = "antigravity",
}

local function by_id(a: registry.Agent): string
	return a.id
end

-- An agent's search text: its name, maker and what it says of itself.
local function matches(a: registry.Agent, q: string): boolean
	if q == "" then
		return true
	end
	local hay = string.lower(`{a.name} {a.by} {a.about} {a.id}`)
	return string.find(hay, q, 1, true) ~= nil
end

-- The registry: every agent, searched, each a row that opens it; those
-- Zeron drives itself say so and stay put, and those added say so.
function M.registry_page(nav: Nav): Node
	local query = ui.state("settings_add_agent_query", "")
	local function q(): string
		return string.lower((string.gsub(query:get(), "^%s*(.-)%s*$", "%1")))
	end
	local listed = ui.computed(function(): { registry.Agent }
		local needle = q()
		local out = {}
		for _, a in registry.now:get().agents do
			if not a.delisted and not a.own and matches(a, needle) then
				table.insert(out, a)
			end
		end
		return out
	end)
	local function count(): number
		local n = 0
		for _, a in registry.now:get().agents do
			if not a.delisted and not a.own then
				n += 1
			end
		end
		return n
	end
	-- Last, one of your own: an agent that isn't in the registry.
	local own_row = ds.Settings.Row {
		label = ui.t("settings.providers.add.own_row"),
		description = ui.t("settings.providers.add.own_row_description"),
		lead = ds.ProviderCard.disc(ds.Icon { name = "terminal", size = 16 }),
		on_click = function()
			nav.go("own")
		end,
	}

	local function row(r: State<registry.Agent>): Node
		local first = r:peek()
		local function control(): string
			local a = r:get()
			if a.built_in then
				return "built_in"
			elseif a.added then
				return if a.added ~= a.version then "update" else "added"
			end
			return "none"
		end
		return ds.Settings.Row {
			label = first.name,
			description = function()
				return r:get().about
			end,
			lead = ds.AgentMark {
				label = first.name,
				seed = first.id,
				mark = if first.built_in then BUILT_IN_MARKS[first.built_in :: string] else nil,
				icon = first.icon,
				size = 32,
			},
			badge = if first.by ~= "" and not first.built_in then ds.Badge { first.by } else nil,
			-- Built in: said where a control goes (the row doesn't open).
			-- Added, or with an update: said before the chevron.
			control = if first.built_in
				then ui.text { ui.t("settings.providers.add.built_in"), text_size = "$font.size.sm", color = "$text.tertiary" }
				else nil,
			value = if first.built_in
				then nil
				else function(): string
					local k = control()
					if k == "added" then
						return ui.t("settings.providers.add.added"):get()
					elseif k == "update" then
						return ui.t("settings.providers.add.update_badge"):get()
					end
					return ""
				end,
			on_click = if first.built_in
				then nil
				else function()
					nav.go(`agent:{first.id}`)
				end,
		}
	end

	local function list(): Node
		return ui.column {
			w_full = true,
			gap = "$space.3",
			ui.show(function()
				return registry.now:get().offline
			end, ds.Hint { ui.t("settings.providers.add.offline") }),
			ui.show(
				function()
					return #listed:get() > 0
				end,
				ds.Settings.Group {
					ui.column {
						w_full = true,
						ui.each(listed, function(_a: registry.Agent, i: number, r: State<registry.Agent>): Node
							return ui.column {
								w_full = true,
								-- (Each brings the rule above it but the first.)
								ui.show(function()
									local l = listed:get()
									return l[1] ~= nil and l[1].id ~= r:get().id
								end, ds.Settings.Rule()),
								row(r),
							}
						end, { key = by_id }),
						ds.Settings.Rule(),
						own_row,
					},
				}
			),
			ui.show(
				function()
					return #listed:get() == 0
				end,
				ds.EmptyState {
					icon = "search",
					title = function(): string
						return ui.t("settings.providers.add.no_match_title", { q = query:get() }):get()
					end,
					body = ui.t("settings.providers.add.no_match_body"),
					size = "sm",
					action = {
						label = ui.t("settings.providers.add.clear"),
						on_click = function()
							query:set("")
						end,
					},
				}
			),
		}
	end

	local function body(state: string): Node
		if state == "looking" then
			return ds.ProviderCard.Loading()
		elseif state == "unavailable" then
			return ds.Callout {
				tone = "danger",
				title = function(): string
					return ui.t("settings.providers.add.unavailable", { device = nav.device() }):get()
				end,
				body = ui.t("settings.providers.add.unavailable_hint"),
				action = {
					label = ui.t("settings.providers.add.try_again"),
					on_click = function()
						registry.load(true)
					end,
				},
			}
		end
		return list()
	end

	return sub_page(
		back(ui.t("settings.providers.title"), "agents", nav),
		ds.Settings.Page {
			title = ui.t("settings.providers.add.title"),
			description = function(): string
				local n = count()
				return if n > 0
					then ui.t("settings.providers.add.description", { n = n }):get()
					else ui.t("settings.providers.add.description_none"):get()
			end,
			trail = ds.SearchField {
				value = query,
				placeholder = ui.t("settings.providers.add.search"),
				name = ui.t("settings.providers.add.search"),
				size = "sm",
				w = 220,
				autofocus = true,
			},
			ui.presence(function(): string
				return registry.now:get().state
			end, body, { w_full = true, enter = { opacity = { 0, 1 }, duration = 100 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
		}
	)
end

-- What adding it takes on the device, as an outcome.
local function runs(a: registry.Agent, device: string): string
	local r, k = a.readiness, a.kind
	if r == "no-build" or not k then
		return ui.t("settings.providers.add.runs.none", { device = device }):get()
	elseif k == "binary" then
		return ui.t("settings.providers.add.runs.binary", { device = device, host = a.host or "" }):get()
	elseif k == "npx" then
		return ui.t(if r == "needs-node" then "settings.providers.add.runs.npx_missing" else "settings.providers.add.runs.npx", { device = device }):get()
	end
	return ui.t(if r == "needs-uv" then "settings.providers.add.runs.uvx_missing" else "settings.providers.add.runs.uvx", { device = device }):get()
end

-- How its download is checked.
local function checked(a: registry.Agent): string
	if a.checksum == true then
		return ui.t("settings.providers.add.checked.sha"):get()
	elseif a.checksum == false then
		return ui.t("settings.providers.add.checked.none"):get()
	elseif a.kind == "npx" then
		return ui.t("settings.providers.add.checked.npm", { version = a.version }):get()
	elseif a.kind == "uvx" then
		return ui.t("settings.providers.add.checked.pypi", { version = a.version }):get()
	end
	return ""
end

-- The facts, read before adding: who, what, where from, and what adding
-- it takes and brings.
local function facts(a: registry.Agent, device: string): Node
	local items: { any } = {}
	if a.by ~= "" then
		table.insert(items, { key = ui.t("settings.providers.add.fact.by"), value = a.by })
	end
	table.insert(items, { key = ui.t("settings.providers.add.fact.version"), value = a.version })
	if a.license then
		table.insert(items, { key = ui.t("settings.providers.add.fact.license"), value = a.license })
	end
	if a.source and a.source_url then
		local url = a.source_url :: string
		table.insert(items, {
			key = ui.t("settings.providers.add.fact.source"),
			value = ds.Link {
				a.source :: string,
				external = true,
				href = url,
				size = "sm",
				tone = "quiet",
				on_click = function()
					host.invoke("open_url", url)
				end,
			},
		})
	end
	table.insert(items, { key = ui.t("settings.providers.add.fact.runs"), value = runs(a, device) })
	local check = checked(a)
	if check ~= "" then
		table.insert(items, { key = ui.t("settings.providers.add.fact.checked"), value = check })
	end
	table.insert(items, { key = ui.t("settings.providers.add.fact.models"), value = ui.t("settings.providers.add.models", { name = a.name }):get() })
	table.insert(items, { key = ui.t("settings.providers.add.fact.sign_in"), value = ui.t("settings.providers.add.sign_in"):get() })
	return ds.Settings.Section {
		ds.Settings.Group {
			ui.column { w_full = true, px = "$space.4", py = "$space.3", ds.KeyValue { items = items, key_width = 84 } },
		},
	}
end

-- Opening an agent's page starts afresh: an add's outcome is for the visit
-- it happened in. (On navigation, not as the page is built: a hot reload
-- builds it again mid-visit.)
function M.visit(id: string)
	local phase = ui.state(`settings_add_agent_phase_{id}`, "idle")
	if phase:peek() ~= "adding" then
		phase:set("idle")
	end
end

-- One agent of the registry, before it is added: what it is, where it is
-- from, what adding it takes, what it may do, and the one button; then
-- adding, added, or the failure, in the same place. Added already: its
-- version there, the update the registry has, and removing it.
function M.agent_page(id: string, nav: Nav): Node
	local first = registry.find(id)
	if not first then
		return sub_page(
			back(ui.t("settings.providers.add.title"), "add", nav),
			ds.EmptyState {
				icon = "search",
				title = ui.t("settings.providers.add.gone_title"),
				body = ui.t("settings.providers.add.gone_body"),
				size = "sm",
			}
		)
	end
	local a = first :: registry.Agent
	local device = nav.device()
	-- What's under way here: nothing, an add (or update), its outcome, or
	-- a removal; `reason` a failure's words.
	local phase = ui.state(`settings_add_agent_phase_{id}`, "idle")
	local reason = ui.state(`settings_add_agent_reason_{id}`, "")
	local removing = ui.state(`settings_add_agent_removing_{id}`, false)
	local function now(): registry.Agent
		return registry.find(id) or a
	end
	local function add()
		phase:set("adding")
		registry.add(id):after(function()
			phase:set("added")
		end, function(err: any)
			local m = if type(err) == "table" then err.message else tostring(err)
			reason:set(if m == "offline" then "" else m or "")
			phase:set("failed")
		end)
	end
	-- Being added (from here, or as the engine says).
	local function busy(): boolean
		return phase:get() == "adding" or now().adding
	end
	-- Which outcome shows: the offer (and the add under way, in the same
	-- place: the button keeps its place and focus), done, failed, or what
	-- the device has.
	local function shown(): string
		local p = phase:get()
		if p == "added" or p == "failed" then
			return p
		end
		local now_a = now()
		if busy() or not now_a.added then
			return "offer"
		end
		return if now_a.added ~= now_a.version then "outdated" else "on_device"
	end
	local function outcome(k: string): Node
		if k == "added" then
			return ds.Callout {
				tone = "success",
				title = ui.t("settings.providers.add.added_title", { name = a.name }),
				body = ui.t("settings.providers.add.added_body", { device = device }),
				action = {
					label = ui.t("settings.providers.add.open_providers"),
					on_click = function()
						nav.go("agents")
					end,
				},
			}
		elseif k == "failed" then
			return ds.Callout {
				tone = "danger",
				title = ui.t("settings.providers.add.failed_title", { name = a.name }),
				body = function(): string
					local why = reason:get()
					return if why == ""
						then ui.t("settings.providers.add.failed_offline", { device = device }):get()
						else ui.t("settings.providers.add.failed_body", { reason = why, device = device }):get()
				end,
				action = {
					label = ui.t("settings.providers.add.try_again"),
					on_click = add,
				},
			}
		elseif k == "on_device" or k == "outdated" then
			local outdated = k == "outdated"
			return ds.Settings.Group {
				ds.Settings.Row {
					label = ui.t("settings.providers.add.on_device", { device = device }),
					description = function(): string
						local n = now()
						return if outdated
							then ui.t("settings.providers.add.outdated", { version = n.added or "", latest = n.version }):get()
							else ui.t("settings.providers.add.installed", { version = n.added or "" }):get()
					end,
					lead = ds.ProviderCard.disc(ds.Icon { name = "check", size = 16 }, "success"),
					control = ui.row {
						gap = "$space.2",
						items = "center",
						ds.Button {
							ui.t("settings.providers.add.remove"),
							tone = "ghost",
							size = "sm",
							on_click = function()
								removing:set(true)
							end,
						},
						if outdated
							then ds.Button {
								ui.t("settings.providers.add.update_to", { version = a.version }),
								tone = "secondary",
								size = "sm",
								on_click = add,
							}
							else nil,
					},
				},
			}
		end
		-- Not added, or being added: what it may do, and the one button.
		local ready = a.readiness == "ready"
		return ui.column {
			w_full = true,
			gap = "$space.3",
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "start",
				px = ds.Settings.inset(),
				color = "$text.tertiary",
				ds.Icon { name = "shield-check", size = 14, mt = 2 },
				ui.text {
					ui.t("settings.providers.add.consent", { name = a.name, device = device }),
					grow = 1,
					min_w = 0,
					wrap = true,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
				},
			},
			ui.row {
				w_full = true,
				justify = "end",
				gap = "$space.3",
				items = "center",
				ui.show(busy, ui.text { ui.t("settings.providers.add.adding_hint"), text_size = "$font.size.sm", color = "$text.tertiary" }),
				ds.Button {
					ui.t("settings.providers.add.add", { device = device }),
					disabled = not ready,
					busy = busy,
					on_click = add,
				},
			},
		}
	end
	return ui.column {
		w_full = true,
		sub_page(
			back(ui.t("settings.providers.add.title"), "add", nav),
			ds.Settings.Page {
				title = a.name,
				description = a.about,
				trail = ds.AgentMark { label = a.name, seed = a.id, icon = a.icon, size = 48 },
				facts(a, device),
				ui.presence(shown, outcome, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
			}
		),
		ds.Dialog {
			open = removing,
			title = ui.t("settings.providers.add.remove_title", { name = a.name, device = device }),
			body = ui.t("settings.providers.add.remove_body"),
			tone = "danger",
			confirm = {
				label = ui.t("settings.providers.add.remove"),
				on_click = function()
					registry.remove(id):after(function()
						phase:set("idle")
					end, function()
						reason:set("")
						phase:set("idle")
					end)
				end,
			},
		},
	}
end

-- One of your own: a name, how to run it, and what it needs from the
-- environment; all of it stays on the device. It runs through the same
-- harness as the registry's agents.
function M.own_page(nav: Nav): Node
	local name = ui.state("settings_own_agent_name", "")
	local command = ui.state("settings_own_agent_command", "")
	local env = ui.state("settings_own_agent_env", "")
	-- "idle", "adding", or "failed" (`why` its words).
	local phase = ui.state("settings_own_agent_phase", "idle")
	local why = ui.state("settings_own_agent_why", "")
	local device = nav.device()
	local function ready(): boolean
		return string.match(name:get(), "%S") ~= nil and string.match(command:get(), "%S") ~= nil and phase:get() ~= "adding"
	end
	local function add()
		if not ready() then
			return
		end
		phase:set("adding")
		registry.add_own(name:peek(), command:peek(), env:peek()):after(function()
			phase:set("idle")
			name:set("")
			command:set("")
			env:set("")
			nav.go("agents")
		end, function(err: any)
			local m = if type(err) == "table" then err.message else tostring(err)
			why:set(if m == "offline" then "" else m or "")
			phase:set("failed")
		end)
	end
	return sub_page(
		back(ui.t("settings.providers.add.title"), "add", nav),
		ds.Settings.Page {
			title = ui.t("settings.providers.add.own.title"),
			description = ui.t("settings.providers.add.own.description", { device = device }),
			ds.Settings.Group {
				ds.Settings.Row {
					label = ui.t("settings.providers.add.own.name"),
					description = ui.t("settings.providers.add.own.name_hint"),
					control = ds.TextField {
						value = name,
						name = ui.t("settings.providers.add.own.name"),
						placeholder = ui.t("settings.providers.add.own.name_placeholder"),
						w = 240,
						autofocus = true,
					},
				},
				ds.Settings.Row {
					label = ui.t("settings.providers.add.own.command"),
					description = ui.t("settings.providers.add.own.command_hint"),
					stacked = true,
					control = ds.TextArea {
						value = command,
						name = ui.t("settings.providers.add.own.command"),
						placeholder = "~/code/halcyon-agent/bin/agent --acp",
						mono = true,
						rows = 1,
						max_rows = 3,
						w_full = true,
					},
				},
				ds.Settings.Row {
					label = ui.t("settings.providers.add.own.env"),
					description = ui.t("settings.providers.add.own.env_hint", { device = device }),
					stacked = true,
					control = ds.TextArea {
						value = env,
						name = ui.t("settings.providers.add.own.env"),
						placeholder = "OPENAI_API_KEY=sk-…",
						mono = true,
						rows = 2,
						max_rows = 6,
						w_full = true,
					},
				},
			},
			ui.show(function()
				return phase:get() == "failed"
			end, ds.Callout {
				tone = "danger",
				title = ui.t("settings.providers.add.own.failed"),
				body = function(): string
					local w = why:get()
					return if w == ""
						then ui.t("settings.providers.add.failed_offline", { device = device }):get()
						else ui.t("settings.providers.add.failed_body", { reason = w, device = device }):get()
				end,
			}),
			ui.row {
				w_full = true,
				justify = "end",
				ds.Button {
					ui.t("settings.providers.add.add", { device = device }),
					disabled = function(): boolean
						return not ready()
					end,
					busy = function(): boolean
						return phase:get() == "adding"
					end,
					on_click = add,
				},
			},
		}
	)
end

return M
