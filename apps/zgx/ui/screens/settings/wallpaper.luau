-- A new chat's picture of your own, as the shipping app's "New thread
-- background" (crates/ui settings.rs, settings/appearance.rs): one image,
-- chosen, replaced, adjusted or removed. The host keeps a copy of it
-- (`wallpaper.keep`), so the original can move or go; replacing or
-- removing it lets that copy go (`wallpaper.forget`).
--
--   wallpaper.choose()       asks for a picture, keeps it, and shows it
--   wallpaper.remove()       no picture (a new chat opens on nothing)
--   wallpaper.missing()      whether the kept copy has gone
--   wallpaper.plays(image)   whether it plays (a video or GIF: Windows)
--   wallpaper.still(image)   what shows it held still: a picture's path, a
--                            video's or GIF's first frame
--   wallpaper.Background(image, props?)  it filling its box: a picture as
--                            ds.wallpaper.Picture lays it, a video or GIF
--                            playing in a web view (WebView2 decodes it on
--                            the GPU: none of its frames pass through the app)
--   wallpaper.adjusting      the Adjust dialog's state (open, its draft)
--   wallpaper.problem_text() what went wrong, in the locale shown ("" nothing)
--   wallpaper.EffectSelect(state)  Zeron's effects, as a select

local ds = require("@ds")
local appearance = require("@app/screens/shell/appearance")
local host = require("@app/screens/settings/host")

local M = {}

-- Zeron's effects as a select (Appearance's over your picture, and on a
-- plugin's settings page over its wallpaper).
function M.EffectSelect(value: State<string>): Node
	return ds.Select {
		value = value,
		name = ui.t("settings.appearance.effect.label"),
		w = 160,
		options = function(): { any }
			local out = {}
			for _, e in ds.wallpaper.EFFECTS do
				table.insert(out, { value = e, label = ui.t(`settings.appearance.effect.{e}.label`):get() })
			end
			return out
		end,
	}
end

-- What went wrong ("" nothing: else its message's key).
M.problem = ui.state("wallpaper_problem", "")
function M.problem_text(): string
	local key = M.problem:get()
	return if key ~= "" then ui.t(key):get() else ""
end

-- What a picture can be: an image, or (Windows) a video, which plays.
local IMAGES = { "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff" }
local VIDEOS = { "mp4", "m4v", "mov", "webm" }

-- (Videos play on Windows alone: the host's platform, asked as the picker
-- opens.)
local function videos(): { string }
	local ok, platform = pcall(function(): any
		return ui.shared("platform", ""):peek()
	end)
	return if ok and platform == "windows" then VIDEOS else {}
end

-- (Kept with the first frame the host read of it, as the one that plays.)
function M.plays(image: any): boolean
	return image ~= nil and type(image.poster) == "string" and image.poster ~= ""
end

function M.still(image: any): string
	return if M.plays(image) then image.poster else image.path
end

-- The player's address for `image` framed at `x`, `y`, `zoom` ("" none):
-- the page beside the kept files, told which and how by its fragment, and
-- (`play`) whether to play: the window shows (not hidden, minimized or
-- covered), so nothing is decoded while no one sees it.
local function player(image: any, x: number, y: number, zoom: number, faded: boolean, play: boolean): string
	local page = ui.call_sync("wallpaper.player")
	if type(page) ~= "string" or page == "" then
		return ""
	end
	local function encoded(text: string): string
		return (string.gsub(text, "[^%w%-%._~/:]", function(c: string): string
			return string.format("%%%02X", string.byte(c))
		end))
	end
	local name = string.match(image.path, "[^/\\]+$") or ""
	local url = "file:///" .. encoded((string.gsub(page, "\\", "/")))
	return `{url}#src={encoded(name)}&x={x}&y={y}&zoom={zoom}&fade={if faded then 1 else 0}&play={if play then 1 else 0}`
end

function M.Background(image: any, props: { x: number?, y: number?, zoom: number?, faded: boolean? }?): Node
	local at = props or {}
	local x, y, zoom = at.x or image.x or 0.5, at.y or image.y or 0.5, at.zoom or image.zoom or 1
	if not M.plays(image) then
		return ds.wallpaper.Picture { src = image.path, w = image.w, h = image.h, x = x, y = y, zoom = zoom }
	end
	return ui.webview {
		-- (A changed fragment is the same page, told again: no reload.)
		src = function(): string
			-- (`window_minimized`: the host's, src/video_wallpaper.rs, where
			-- zgx doesn't say so itself.)
			local shows = ui.window.shown:get() and not ui.shared("window_minimized", false):get()
			return player(image, x, y, zoom, at.faded ~= false, shows)
		end,
		transparent = true,
		profile = "ephemeral",
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		pointer_events = "none",
		aria_hidden = true,
	}
end

-- Whether the picture kept has gone (its file removed by hand).
function M.missing(): boolean
	local image = appearance.image:get()
	if image == nil then
		return false
	end
	if M.plays(image) then
		return not ui.call_sync("wallpaper.exists", { path = image.path })
	end
	-- (Its header read: nothing there, or not a picture.)
	local ok, w = pcall(ui.image_size, image.path)
	return not ok or w == nil
end

function M.choose()
	host.open_files({
		title = ui.t("settings.appearance.wallpaper.choose_title"):get(),
		filters = {
			{ ui.t("settings.appearance.wallpaper.images"):get(), table.move(videos(), 1, #videos(), #IMAGES + 1, table.clone(IMAGES)) },
		},
	}):after(function(files: { File })
		local f = files[1]
		if not f then
			return
		end
		host.call("wallpaper.keep", { path = f.path }):after(function(kept: any)
			local was = appearance.image:peek()
			ui.batch(function()
				M.problem:set("")
				appearance.image:set({
					path = kept.path,
					name = kept.name,
					w = kept.w,
					h = kept.h,
					x = 0.5,
					y = 0.5,
					zoom = 1,
					poster = if kept.poster ~= "" then kept.poster else nil,
				})
				appearance.choose("image")
			end)
			if was and was.path ~= kept.path then
				host.invoke("wallpaper.forget", { path = was.path })
			end
		end, function(err: any)
			local word = if type(err) == "table" then err.message else nil
			M.problem:set(if word == "invalid" then "settings.appearance.wallpaper.unsupported" else "settings.appearance.wallpaper.couldnt_keep")
		end)
	end)
end

function M.remove()
	local was = appearance.image:peek()
	ui.batch(function()
		M.problem:set("")
		appearance.image:set(nil)
		if appearance.wallpaper:peek() == "image" then
			appearance.wallpaper:set("none")
		end
	end)
	if was then
		host.invoke("wallpaper.forget", { path = was.path })
	end
end

-- The Adjust dialog: open, and where the picture stands while it's moved
-- (kept only on Done).
M.adjusting = ui.state("wallpaper_adjusting", false)
M.draft = ui.state("wallpaper_adjust_draft", { x = 0.5, y = 0.5, zoom = 1 })

function M.adjust()
	local image = appearance.image:peek()
	if not image then
		return
	end
	ui.batch(function()
		M.draft:set({ x = image.x, y = image.y, zoom = image.zoom })
		M.adjusting:set(true)
	end)
end

function M.keep_adjustment()
	local image = appearance.image:peek()
	if image then
		local d = M.draft:peek()
		local next = table.clone(image)
		next.x, next.y, next.zoom = d.x, d.y, d.zoom
		appearance.image:set(next)
	end
	M.adjusting:set(false)
end

return M
