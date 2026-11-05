-- A new chat's picture of your own, as the shipping app's "New thread
-- background" (crates/ui settings.rs, settings/appearance.rs): one image,
-- chosen, replaced, adjusted or removed. The host keeps a copy of it
-- (`wallpaper.keep`), so the original can move or go; replacing or
-- removing it lets that copy go (`wallpaper.forget`).
--
--   wallpaper.choose()       asks for a picture, keeps it, and shows it
--   wallpaper.remove()       no picture (a new chat opens on nothing)
--   wallpaper.missing()      whether the kept copy has gone
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

-- Whether the picture kept has gone (its file removed by hand).
function M.missing(): boolean
	local image = appearance.image:get()
	if image == nil then
		return false
	end
	-- (Its header read: nothing there, or not a picture.)
	local ok, w = pcall(ui.image_size, image.path)
	return not ok or w == nil
end

function M.choose()
	host.open_files({
		title = ui.t("settings.appearance.wallpaper.choose_title"):get(),
		filters = { { ui.t("settings.appearance.wallpaper.images"):get(), { "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff" } } },
	}):after(function(files: { File })
		local f = files[1]
		if not f then
			return
		end
		host.call("wallpaper.keep", { path = f.path }):after(function(kept: any)
			local was = appearance.image:peek()
			ui.batch(function()
				M.problem:set("")
				appearance.image:set({ path = kept.path, name = kept.name, w = kept.w, h = kept.h, x = 0.5, y = 0.5, zoom = 1 })
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
