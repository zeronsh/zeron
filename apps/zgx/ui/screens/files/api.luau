-- A session's files, as the host reaches them (src/files.rs): its folders
-- listed one at a time, its files read, saved, made, moved and deleted, and
-- what changes on disk. Each call is a promise (`:after(on_value, on_error)`);
-- an error's message is "offline" when Zeron can't be reached, else a line
-- for the screen. Tests stub these functions (ui/files_fixture.luau).
--
--   api.list(target, dir)          { checkout, can_move, can_delete, truncated,
--                                    entries = { Entry } }
--   api.read(target, path)         File
--   api.write(target, request)     { status = "written", hash, size }
--                                  or { status = "conflict", reason, hash? }
--   api.move(target, request)      { status = "applied" }
--   api.delete(target, request)    or { status = "rejected", message }
--   api.search(target, query)      { { path, dir } }, best first
--   api.create(target, path, dir, checkout)
--   api.reveal(target, path), api.open_external(target, path)   (this Mac's)
--   api.watch(key, target)         its changes from now on, in `api.events`
--   api.unwatch(key)
--   api.frames(key, since)         a folder's kept frames past `since`, now
--
-- `api.events:get()` is `{ [key] = next }`: each watched folder's next
-- frame number, `n` counting up per folder. Its frames stay with the host
-- until asked for (`api.frames`).

local M = {}

-- Where the files are: a chat's folder, or a project's, on its device.
export type Target = { chat: string?, space: string?, device: string?, root: string? }

export type Entry = {
	name: string,
	path: string,
	dir: boolean,
	kind: string,
	size: number?,
	ignored: boolean,
	readonly: boolean,
	rev: string?,
}
export type Listing = { checkout: string?, can_move: boolean, can_delete: boolean, here: boolean?, truncated: boolean, entries: { Entry } }

export type File = {
	checkout: string?,
	path: string,
	text: string?,
	hash: string?,
	size: number?,
	-- "utf8", "utf8Bom", "binary", "unsupported".
	encoding: string?,
	line_ending: string?,
	-- Why it can't be saved: "binary", "tooLarge", "mixedLineEndings", …
	readonly: string?,
	truncated: boolean?,
	-- On this Mac (an image shows from its path).
	here: boolean?,
}

export type Change = { kind: string, path: string, old_path: string? }
export type Frame = { n: number, key: string, sequence: number?, resync: boolean, changes: { Change } }
export type Events = { [string]: number }

M.events = ui.shared("files.events", {} :: Events)
-- Git's statuses for each watched folder, `{ [key] = { [path] = status } }`
-- (src/files.rs, beside its watch).
M.git = ui.shared("files.git", {} :: { [string]: { [string]: string } })

function M.frames(key: string, since: number): { Frame }
	return ui.call_sync("files.frames", { key = key, since = since }) or {}
end

-- Lines holding `query` in the folder (the palette's "In Files"): `{
-- matches = { { path, line, column, text, ranges } }, truncated, indexing
-- }`, best first. A newer search rejects an older one's ("superseded").
function M.search_content(target: Target, query: string, opts: { limit: number?, per_file: number? }?): any
	return ui.call("files.search_content", {
		target = target,
		query = query,
		limit = opts and opts.limit,
		per_file = opts and opts.per_file,
	})
end

function M.list(target: Target, dir: string): any
	return ui.call("files.list", { target = target, dir = dir })
end

function M.read(target: Target, path: string): any
	return ui.call("files.read", { target = target, path = path })
end

export type Write = { path: string, text: string, hash: string, checkout: string, encoding: string?, line_ending: string? }
function M.write(target: Target, request: Write): any
	local arg: { [string]: any } = table.clone(request :: any)
	arg.target = target
	return ui.call("files.write", arg)
end

export type Move = { from: string, to: string, rev: string, kind: string, checkout: string }
function M.move(target: Target, request: Move): any
	local arg: { [string]: any } = table.clone(request :: any)
	arg.target = target
	return ui.call("files.move", arg)
end

export type Delete = { path: string, rev: string, kind: string, checkout: string, recursive: boolean? }
function M.delete(target: Target, request: Delete): any
	local arg: { [string]: any } = table.clone(request :: any)
	arg.target = target
	return ui.call("files.delete", arg)
end

-- Paths whose names match `query`, best first (folders not read yet too).
function M.search(target: Target, query: string): any
	return ui.call("files.search", { target = target, query = query })
end

-- `{ status = "applied" }` or `{ status = "rejected", message }`.
function M.create(target: Target, path: string, dir: boolean, checkout: string): any
	return ui.call("files.create", { target = target, path = path, dir = dir, checkout = checkout })
end

-- On this Mac only: shown in Finder, or opened in the app it opens in.
-- An image on another device, its bytes read from there: `{ src, size }`,
-- `src` a file of them here (src/files.rs).
function M.image(target: Target, path: string, checkout: string): any
	return ui.call("files.image", { target = target, path = path, checkout = checkout })
end

function M.reveal(target: Target, path: string): any
	return ui.call("files.reveal", { target = target, path = path })
end

function M.open_external(target: Target, path: string): any
	return ui.call("files.open_external", { target = target, path = path })
end

-- `{ kept }`: whether that watch was running already (kept a moment after
-- being let go of), every change since in its frames.
function M.watch(key: string, target: Target): any
	return ui.call("files.watch", { key = key, target = target })
end

function M.unwatch(key: string)
	ui.invoke("files.unwatch", key)
end

return M
