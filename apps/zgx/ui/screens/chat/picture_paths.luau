-- Paths on a chat's device, whichever that is (crates/ui tool_images.rs's
-- rules): a Mac's or Linux's `/x`, or Windows' `C:\x`. The chat's device may
-- be Windows while this one isn't, or the other way round, so both are
-- read everywhere. No `ui` here: plain functions.

local M = {}

-- `http:`, `file:`, `data:`, `javascript:`…; a drive letter (`C:\`) is not one.
function M.has_scheme(src: string): boolean
	local scheme = string.match(src, "^([%a][%w%+%-%.]*):")
	return scheme ~= nil and #scheme > 1
end

-- Whether `path` is absolute on its device: `/x`, `\x`, or a drive's
-- `C:\x` / `C:/x`.
function M.absolute(path: string): boolean
	return string.match(path, "^[/\\]") ~= nil or string.match(path, "^%a:[/\\]") ~= nil
end

-- Network and device paths, never read: `//host/x`, `\\host\x`, `\\.\x`,
-- `\??\x`, and a drive's current folder (`C:x`).
function M.remote_or_device(path: string): boolean
	return string.match(path, "^[/\\][/\\]") ~= nil
		or string.sub(path, 1, 4) == "\\??\\"
		or (string.match(path, "^%a:") ~= nil and string.match(path, "^%a:[/\\]") == nil)
end

-- `path` absolute on its device: as it is, or joined to `cwd` (which must be
-- absolute itself) as that folder's device writes paths, `\` on Windows.
-- Nil when there's no folder to join it to.
function M.resolve(path: string, cwd: string?): string?
	if M.absolute(path) then
		return path
	end
	if not cwd or not M.absolute(cwd) then
		return nil
	end
	local rel = string.gsub(path, "^%./", "")
	if string.find(cwd, "\\", 1, true) then
		return (string.gsub(cwd, "[/\\]+$", "")) .. "\\" .. (string.gsub(rel, "/", "\\"))
	end
	return (string.gsub(cwd, "/+$", "")) .. "/" .. rel
end

local RASTER = { png = true, jpg = true, jpeg = true, webp = true, gif = true }

-- The path of a picture a reply names, in a chat whose folder is `cwd`, or
-- nil when it isn't one to draw (the shipping app's `markdown_image`): a
-- PNG, JPEG, WebP or GIF by its name, never a URL, a home path, a network
-- or device path.
function M.picture(src: string, cwd: string?): string?
	src = string.match(src, "^%s*(.-)%s*$") or ""
	if src == "" or string.sub(src, 1, 1) == "~" or M.has_scheme(src) or M.remote_or_device(src) then
		return nil
	end
	local ext = string.lower(string.match(src, "%.([%w]+)$") or "")
	if not RASTER[ext] then
		return nil
	end
	return M.resolve(src, cwd)
end

return M
