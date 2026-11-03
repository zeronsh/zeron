-- The pictures a reply names (`![alt](path)`), as Zeron's transcript draws
-- them (crates/ui/src/reply_images.rs): only a path on the chat's own device,
-- absolute or in the chat's folder, of a PNG, JPEG or WebP by its name. No
-- URL is ever fetched (`http:`, `file:`, `data:`…), and `~`, network and
-- device paths are refused: those stay what the reply wrote (their alt
-- text). The engine reads each from strictly inside the chat's folder,
-- never through a link (`files.chat_image`), and it is kept here as a file
-- named by its bytes.
--
-- What each picture is (coming, shown, couldn't be read) is kept by chat
-- and path in state that outlives the view: a reload, a chat opened again,
-- the same picture in another reply, none reads it twice. At most four are
-- read at once, as Zeron's. One that couldn't be read is read again, as
-- Zeron's shared backoff does (a picture an agent names a moment before it
-- writes the file): 3, 10, 30 and 90 seconds on, while it shows
-- (`retry`, the time it's due, which ds/reply_picture waits for).
--
-- (A GIF plays as zgx's image views play one: still on its first frame
-- while the system asks for reduced motion.)
--
--   local pictures = ReplyImages.for_session(function() return store.session:get() end)
--   Transcript { pictures = pictures, … }

local M = {}

-- (`path`: where it is on the chat's device; `here`: whether that's this
-- Mac, so Show in Finder can find it; `tries` and `retry`: reads that
-- failed, and when the next is due, in `ui.now()` milliseconds.)
export type Picture = {
	state: "loading" | "ready" | "failed",
	src: string?,
	error: string?,
	path: string?,
	here: boolean?,
	tries: number?,
	retry: number?,
}
export type Pictures = {
	-- What `src` is in the open chat: nil when it isn't a picture to draw.
	resolve: (src: string) -> Picture?,
	-- Read it, unless it has been (or is being).
	fetch: (src: string) -> (),
}
type Session = { id: string, device: string?, cwd: string? }

-- Every picture, by `device|chat|path`.
local PICTURES = ui.state("reply_images", {} :: { [string]: Picture })
-- Reads at once, and what waits.
local LOADS = 4
-- Read again this long after each failed read (ms), then no more.
local BACKOFF = { 3000, 10000, 30000, 90000 }
local running = 0
local waiting: { () -> () } = {}

-- How a picture is read: the host's action (a test puts its own here).
M.read = function(target: { chat: string, device: string? }, path: string): any
	return ui.call("files.chat_image", { target = target, path = path })
end

-- `http:`, `file:`, `data:`, `javascript:`…; a drive letter (`C:\`) is not one.
local function has_scheme(src: string): boolean
	local scheme = string.match(src, "^([%a][%w%+%-%.]*):")
	return scheme ~= nil and #scheme > 1
end

local RASTER = { png = true, jpg = true, jpeg = true, webp = true, gif = true }

-- The path of a picture a reply names, in a chat whose folder is `cwd`, or
-- nil when it isn't one to draw.
function M.path(src: string, cwd: string?): string?
	src = string.match(src, "^%s*(.-)%s*$") or ""
	if src == "" or string.sub(src, 1, 1) == "~" or has_scheme(src) then
		return nil
	end
	-- (Network and device paths: `//host/x`, `\\host\x`, `\\.\x`.)
	if string.sub(src, 1, 2) == "//" or string.sub(src, 1, 2) == "\\\\" then
		return nil
	end
	local ext = string.lower(string.match(src, "%.([%w]+)$") or "")
	if not RASTER[ext] then
		return nil
	end
	if string.sub(src, 1, 1) == "/" then
		return src
	end
	-- Relative: in the chat's folder, which must be a path of its own.
	if not cwd or string.sub(cwd, 1, 1) ~= "/" then
		return nil
	end
	local rel = string.gsub(src, "^%./", "")
	return (string.gsub(cwd, "/+$", "")) .. "/" .. rel
end

local function key_of(s: Session, path: string): string
	return `{s.device or "here"}|{s.id}|{path}`
end

local function put(key: string, picture: Picture)
	local now = table.clone(PICTURES:peek())
	now[key] = picture
	PICTURES:set(now)
end

local function next_read()
	while running < LOADS and #waiting > 0 do
		local go = table.remove(waiting, 1) :: () -> ()
		go()
	end
end

-- What reads a picture: (target, path) to a promise of `{ src }`.
export type Read = (target: { chat: string, device: string? }, path: string) -> any

-- Pictures of the chat `session()` says is open: `path_of` says where on
-- its device a `src` is (nil: not one to draw), `read` reads one, and
-- `here_ok` says a chat on this device needs no read (the path is here).
local function provider(session: () -> Session?, read: Read, path_of: (src: string, s: Session) -> string?, here_ok: boolean): Pictures
	local function find(src: string): (Session?, string?)
		local s = session()
		if not s or not s.id then
			return nil, nil
		end
		return s, path_of(src, s)
	end
	-- (On this device: the path is the picture.)
	local function local_to(s: Session): boolean
		local here = (s :: any).here
		return here_ok and (s.device == nil or s.device == "" or s.device == here)
	end
	-- (On this Mac, the chat: its paths are files here.)
	local function on_this_mac(s: Session): boolean
		local here = (s :: any).here
		return s.device == nil or s.device == "" or s.device == here
	end
	return {
		resolve = function(src: string): Picture?
			local s, path = find(src)
			if not s or not path then
				return nil
			end
			local here = on_this_mac(s)
			if local_to(s) then
				return { state = "ready", src = path, path = path, here = true }
			end
			local p = PICTURES:get()[key_of(s, path)]
			if not p then
				return { state = "loading", path = path, here = here }
			end
			-- (Where it is, said beside what's kept of it.)
			local out = table.clone(p) :: any
			out.path, out.here = path, here
			return out
		end,
		fetch = function(src: string)
			local s, path = find(src)
			if not s or not path or local_to(s) then
				return
			end
			local key = key_of(s, path)
			local was = PICTURES:peek()[key]
			-- (Read, or being read; failed, read again once it's due.)
			if was and not (was.state == "failed" and was.retry and ui.now() >= was.retry) then
				return
			end
			local tries = if was then was.tries or 0 else 0
			-- (A retry keeps its failure shown until it lands: no flash of
			-- the placeholder each time.)
			if not was then
				put(key, { state = "loading" })
			else
				local still = table.clone(was) :: any
				still.retry = nil
				put(key, still)
			end
			local target = { chat = s.id, device = s.device }
			table.insert(waiting, function()
				running += 1
				local function done(picture: Picture)
					running -= 1
					if picture.state == "failed" then
						tries += 1
						picture.tries = tries
						local wait = BACKOFF[tries]
						picture.retry = if wait then ui.now() + wait else nil
					end
					put(key, picture)
					next_read()
				end
				local ok, promise = pcall(read, target, path :: string)
				if not ok or not promise then
					done({ state = "failed", error = if ok then ui.t("chat.reply_images.couldnt_read"):get() else tostring(promise) })
					return
				end
				promise:after(function(got: any)
					if type(got) == "table" and type(got.src) == "string" then
						done({ state = "ready", src = got.src })
					else
						done({ state = "failed", error = ui.t("chat.reply_images.couldnt_read"):get() })
					end
				end, function(err: any)
					done({ state = "failed", error = tostring(err) })
				end)
			end)
			next_read()
		end,
	}
end

-- The pictures of the chat `session()` says is open, read by `read` (the
-- host's, unless a gallery or a test brings its own).
function M.for_session(session: () -> Session?, read: Read?): Pictures
	return provider(session, read or M.read, function(src: string, s: Session): string?
		return M.path(src, s.cwd)
	end, false)
end

-- How a sent picture is read from the chat's device: the host's action.
M.read_attachment = function(target: { chat: string, device: string? }, path: string): any
	return ui.call("files.attachment", { target = target, path = path })
end

-- The pictures sent with messages (each a path on the chat's device): in a
-- chat on this device, the path itself; on another (`session()`'s `device`
-- not its `here`), read from there through the engine (`files.attachment`,
-- Zeron's transcript thumbnails), kept as reply pictures are.
function M.for_attachments(session: () -> Session?, read: Read?): Pictures
	-- (Any path here shows as it is; one read from another device must be
	-- a path of its own there.)
	return provider(session, read or M.read_attachment, function(src: string, _s: Session): string?
		return if src ~= "" then src else nil
	end, true)
end

-- Forgets every picture (a test's fresh start).
function M.forget()
	PICTURES:set({})
	running = 0
	waiting = {}
end

return M
