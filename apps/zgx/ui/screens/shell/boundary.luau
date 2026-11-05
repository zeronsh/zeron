-- Where one screen ends and the window goes on: each part of the main
-- window (the sidebar, the title bar, the page; in the page, the chat page,
-- the files pane, settings) is loaded and built through here, so a module
-- that fails (a syntax error saved mid-edit, a template that throws) shows
-- what went wrong in its own place and the rest of the window keeps
-- working. Saving it again, fixed, brings it back (the scripts reload).
--
--   boundary.screen("Files", "@app/screens/files", function(files) return files.Pane { … } end)
--   boundary.load("@app/screens/chat")      the module, or nil and why
--   boundary.Failed { name = "Files", error = "…" }
--
-- What's lost is the view alone: where it was (the route, the session, a
-- draft, the sidebar's scroll and folds) is kept outside the screens, in
-- named states and @account's, so a screen made again comes back as it
-- was. A plugin system replacing screens (user templates) replaces this
-- module to show its own error and fall back to the built-in.

local ds = require("@ds")
local locale = require("@ds/locale")

local M = {}

-- What failed, by module, for tools and a plugin's error view.
M.failures = ui.state("shell_failures", {} :: { [string]: string })

local function failed(module: string, why: string?)
	-- (Copied only when it changes.)
	if M.failures:peek()[module] == why then
		return
	end
	local now = table.clone(M.failures:peek())
	now[module] = why
	M.failures:set(now)
end

-- The module at `module`, or nil and why it didn't load.
function M.load(module: string): (any, string?)
	local ok, found = pcall(function()
		return (require :: any)(module)
	end)
	if ok then
		failed(module, nil)
		return found, nil
	end
	local why = tostring(found)
	print(`zeron-zgx: {module} didn't load: {why}`)
	failed(module, why)
	return nil, why
end

-- Who decides what shows in place of a failed module, asked in order
-- before `Failed` shows (the plugin system: a templated screen falls back
-- to Zeron's own at once). The first node one returns shows instead.
M.fallbacks = {} :: { (module: string, why: string) -> Node? }
function M.on_fail(f: (module: string, why: string) -> Node?)
	table.insert(M.fallbacks, f)
end

-- What shows for `module` failing: a fallback's node, else `Failed`.
local function instead(name: Name, module: string, why: string): Node
	for _, f in M.fallbacks do
		local ok, node = pcall(f, module, why)
		if ok and node ~= nil then
			return node
		end
	end
	return M.Failed { name = name, error = why }
end

-- A screen's name: a string, or a function saying it in the locale shown
-- (`named(key)`'s).
export type Name = string | () -> string
export type FailedProps = { name: Name, error: string?, [any]: any }

-- The name a catalog key says, read where it's drawn.
function M.named(key: string): () -> string
	return function(): string
		return locale.s(key)
	end
end

-- A screen that didn't load, in its place.
function M.Failed(props: FailedProps): Node
	return ui.column {
		w_full = true,
		h_full = true,
		min_w = 0,
		items = "center",
		justify = "center",
		p = "$space.4",
		ds.EmptyState {
			icon = "danger-triangle",
			title = ui.t("shell.boundary.failed.title", { name = props.name }) :: any,
			body = ui.t("shell.boundary.failed.body") :: any,
		},
	}
end

-- Each screen's build, by module: `rebuild` moves one on, and that screen
-- (each place it shows) is made again from what its module now is, the
-- rest of the window as it was. (The plugin system's: a template that
-- failed, after mount too, swapped for Zeron's own and shown in place.)
local generations = ui.state("shell_boundary_generations", {} :: { [string]: number })
function M.rebuild(module: string)
	local now = table.clone(generations:peek())
	now[module] = (now[module] or 0) + 1
	generations:set(now)
end
-- The screens showing now, by module: how many places each shows in.
M.mounted = ui.state("shell_boundary_mounted", {} :: { [string]: number })
local function count(module: string, by: number)
	local now = table.clone(M.mounted:peek())
	local n = (now[module] or 0) + by
	now[module] = if n > 0 then n else nil
	M.mounted:set(now)
end

-- One build of a screen: its module loaded and built by `build`, or what
-- failed in its place; counted among those mounted while it shows.
type ScreenProps = { name: Name, module: string, build: (any) -> Node }
local Screen = ui.component(function(props: ScreenProps): Node
	local name, module = props.name, props.module
	count(module, 1)
	ui.on_cleanup(function()
		count(module, -1)
	end)
	local found, why = M.load(module)
	if found == nil then
		return instead(name, module, why or "it didn't load")
	end
	local ok, node = pcall(props.build, found)
	if ok then
		return node
	end
	local reason = tostring(node)
	print(`zeron-zgx: {module} didn't build: {reason}`)
	failed(module, reason)
	return instead(name, module, reason)
end)

-- `module` loaded and built by `build`, or what failed in its place; made
-- again as `rebuild` asks. (A switch lays out as what it shows, so a
-- screen sits in its place as if nothing held it; it swaps between two
-- builds, the next one made afresh, as its module's build moves on.)
function M.screen(name: Name, module: string, build: (any) -> Node, opts: { pass: boolean? }?): Node
	local function one(): Node
		return Screen { name = name, module = module, build = build }
	end
	-- (A screen laid over others that lets the pointer through, as a title
	-- bar does: built as it is, as the switch's box would take the pointer
	-- over all it spans. Made again by a reload of the scripts.)
	if opts and opts.pass then
		return one()
	end
	return ui.switch(function(): number
		return (generations:get()[module] or 0) % 2
	end, { [0] = one(), [1] = one() })
end

return M
