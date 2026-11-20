-- Edit project: a project's name and its icon, from its header's right
-- click (the sidebar's "Edit project…"). One dialog for the window.
--
-- Its tile stands large at the head, as it will be everywhere (the sidebar,
-- the panel, the pickers, the palette), following every change; its name
-- beside it. Under them, what the tile wears:
--   Default  its repository's own logo (found as the shipping app finds it)
--            else its initial on a colour;
--   Emoji    one picked (search, recents, kinds, skin tone), on a wash of
--            the colour;
--   Image    a picture of yours (chosen, or dropped on the dialog), placed
--            in the tile's square by dragging and zooming, as Account's
--            photo is, cut here and kept by Zeron's edge.
-- What's chosen is the project's on every device (sessions.set_project_icon).
--
--   editor.open(id, start?, also?)  asks about that project (`also`: other
--                    projects, by id, given what's saved too: Settings →
--                    Projects' one row for a repository on Cloud's machines)
--   editor.Dialog()   the dialog, placed once in the window

local ds = require("@ds")
local sessions = require("@account/sessions")
local projects = require("@account/projects")
local look = require("@app/screens/shell/project_look")
local locale = require("@ds/locale")

local M = {}

-- What's being edited: the project, its name, which kind of icon, the
-- emoji, the colour (nil: its own), an image already its (an https URL),
-- and a picture chosen now, to place and cut: { path, w, h }.
type Picture = { path: string, w: number, h: number }
local open = ui.state("project_edit_open", false)
local editing = ui.state("project_edit_id", nil :: string?)
local also = ui.state("project_edit_also", {} :: { string })
local name = ui.state("project_edit_name", "")
local kind = ui.state("project_edit_kind", "default")
local emoji = ui.state("project_edit_emoji", nil :: string?)
local hue = ui.state("project_edit_hue", nil :: string?)
local image = ui.state("project_edit_image", nil :: string?)
local picture = ui.state("project_edit_picture", nil :: Picture?)
-- Where the square is in the picture: its centre (0 to 1 each way) and how
-- far in (1: the picture's short side fills it).
local zoom = ui.state("project_edit_zoom", 1)
local centre = ui.state("project_edit_centre", { x = 0.5, y = 0.5 })
local busy = ui.state("project_edit_busy", false)
-- What went wrong, as an error's key (shell.sidebar.project.edit.error.*),
-- and whether the name is wanted.
local failed = ui.state("project_edit_failed", nil :: string?)
local unnamed = ui.state("project_edit_unnamed", false)
local dropping = ui.state("project_edit_dropping", false)
-- A picture uploaded while the dialog was open and not worn yet (its save
-- refused): let go of if the dialog closes without it.
local uploaded = ui.state("project_edit_uploaded", nil :: string?)
ui.effect(function()
	if open:get() then
		return
	end
	local url = uploaded:peek()
	if url then
		uploaded:set(nil)
		projects.forget_icon(url)
	end
end)
-- The emoji picker's own: what was picked lately, and the skin tone.
local recents = ui.persist("project_edit_emoji_recents", {} :: { string })
local tone = ui.persist("project_edit_emoji_tone", 0)

-- The tile's side at the head, and the square a picture is placed in.
local PREVIEW = 64
local VIEW = 184
-- The stage the square stands in (the panel's width), and how tall.
local STAGE_H = 232
-- A colour's swatch, and the ring round the one chosen.
local SWATCH = 18
local RING = 26

local function project(): sessions.Project?
	local id = editing:get()
	for _, p in sessions.projects:get() do
		if p.id == id then
			return p
		end
	end
	return nil
end

-- The colour shown: the one chosen, else the project's own.
local function shown_hue(): string
	local h = hue:get()
	if h then
		return h
	end
	local id = editing:get()
	return if id then look.hue(id) else "violet"
end

-- `start` opens it somewhere else than as the project is (a fixture's
-- captures): `{ kind?, emoji?, hue?, picture?, zoom?, centre? }`.
function M.open(id: string, start: { [string]: any }?, others: { string }?)
	local p: sessions.Project? = nil
	for _, q in sessions.now:peek().projects do
		if q.id == id then
			p = q
		end
	end
	if not p then
		return
	end
	local icon = p.icon or {}
	ui.batch(function()
		editing:set(id)
		also:set(others or {})
		name:set(p.name)
		emoji:set(icon.emoji)
		image:set(icon.image)
		hue:set(icon.hue)
		picture:set(nil)
		zoom:set(1)
		centre:set({ x = 0.5, y = 0.5 })
		kind:set(if icon.image then "image" elseif icon.emoji then "emoji" else "default")
		busy:set(false)
		failed:set(nil)
		unnamed:set(false)
		dropping:set(false)
		if start then
			kind:set(start.kind or kind:peek())
			emoji:set(start.emoji or emoji:peek())
			hue:set(start.hue or hue:peek())
			picture:set(start.picture)
			zoom:set(start.zoom or 1)
			centre:set(start.centre or { x = 0.5, y = 0.5 })
		end
		open:set(true)
	end)
end

-- A picture to place: its size read, then the square starting in its
-- middle, filling it.
local function take(path: string)
	failed:set(nil)
	projects.picture_size(path):after(function(size: any)
		ui.batch(function()
			picture:set({ path = path, w = math.max(1, size.w), h = math.max(1, size.h) })
			zoom:set(1)
			centre:set({ x = 0.5, y = 0.5 })
			kind:set("image")
		end)
	end, function()
		failed:set("open")
	end)
end

local function choose()
	ui.open_files({
		title = ui.t("shell.sidebar.project.edit.choose"):get(),
		filters = { { ui.t("shell.sidebar.project.edit.kind.image"):get(), { "png", "jpg", "jpeg" } } },
	}):after(function(files: { File })
		local f = files[1]
		if f then
			take(f.path)
		end
	end)
end

-- Files carried over the dialog: the first picture among them is taken.
local function dropped(files: { any })
	dropping:set(false)
	for _, f in files do
		local path = if type(f) == "table" then f.path else f
		if type(path) == "string" and (string.match(string.lower(path), "%.png$") or string.match(string.lower(path), "%.jpe?g$")) then
			take(path)
			return
		end
	end
	failed:set("open")
end

-- Points one of the picture's pixels takes in a square `side` wide.
local function scale(side: number): number
	local px = picture:get()
	if not px then
		return 1
	end
	return side * zoom:get() / math.min(px.w, px.h)
end
-- A centre moved back to where the picture still fills the square.
local function placed(x: number, y: number): (number, number)
	local px = picture:peek()
	if not px then
		return 0.5, 0.5
	end
	local k = zoom:peek() / math.min(px.w, px.h)
	local hx = 1 / (2 * px.w * k)
	local hy = 1 / (2 * px.h * k)
	return math.clamp(x, hx, 1 - hx), math.clamp(y, hy, 1 - hy)
end
local function settle()
	local c = centre:peek()
	local x, y = placed(c.x, c.y)
	centre:set({ x = x, y = y })
end
-- The square as the host cuts it: its corner and side, in pixels.
local function crop(): { x: number, y: number, side: number }
	local px = picture:peek() :: Picture
	local c = centre:peek()
	local side = math.min(px.w, px.h) / zoom:peek()
	return { x = c.x * px.w - side / 2, y = c.y * px.h - side / 2, side = side }
end

-- The picture placed in a square `side` wide (the stage's, the preview's):
-- drawn whole at its zoom, the square's part lined up on it.
local function placed_picture(side: number): Node
	return ui.image {
		src = function(): string
			local px = picture:get()
			return if px then px.path else ""
		end,
		absolute = true,
		left = function(): number
			local px = picture:get()
			return if px then side / 2 - centre:get().x * px.w * scale(side) else 0
		end,
		top = function(): number
			local px = picture:get()
			return if px then side / 2 - centre:get().y * px.h * scale(side) else 0
		end,
		w = function(): number
			local px = picture:get()
			return if px then px.w * scale(side) else 0
		end,
		h = function(): number
			local px = picture:get()
			return if px then px.h * scale(side) else 0
		end,
		aria_hidden = true,
	}
end

-- A tile's corner at `side`, in the proportion ds/tile gives its preview
-- (the large radius at 64), so the square a picture is placed in is the
-- tile it will be.
local function corner(side: number): number
	local r = ui.theme.get("radius.xl") :: number
	return math.floor(r * side / PREVIEW + 0.5)
end

-- The head: the tile, large, as it will be, and the name beside it.
local function head(): Node
	local function tile(): Node
		return ui.switch(function(): string
			return if kind:get() == "image" and picture:get() then "placing" else "tile"
		end, {
			-- A picture being placed: the square of it as it's placed.
			placing = ui.div {
				w = PREVIEW,
				h = PREVIEW,
				shrink = 0,
				relative = true,
				overflow = "hidden",
				rounded = "$radius.xl",
				bg = "$zeron.secondary.bg",
				placed_picture(PREVIEW),
			},
			tile = ds.Tile {
				size = PREVIEW,
				label = function(): string?
					return name:get()
				end,
				hue = shown_hue,
				emoji = function(): string?
					return if kind:get() == "emoji" then emoji:get() else nil
				end,
				src = function(): string?
					local k = kind:get()
					if k == "image" then
						return look.file(image:get())
					elseif k == "default" then
						local p = project()
						return if p then look.artwork(p) else nil
					end
					return nil
				end,
			},
		})
	end
	local function where(): string
		local p = project()
		if not p then
			return ""
		end
		local device = ""
		for _, d in sessions.devices:get() do
			if d.id == p.device then
				device = d.name
			end
		end
		return if device ~= "" then `{device} · {p.path}` else p.path
	end
	return ui.row {
		w_full = true,
		gap = "$space.4",
		items = "center",
		ui.div {
			id = "project-edit-preview",
			role = "img",
			label = ui.t("shell.sidebar.project.edit.preview"),
			shrink = 0,
			rounded = "$radius.xl",
			outline = function(): number
				return if dropping:get() then 2 else 0
			end,
			outline_offset = 3,
			outline_color = "$accent",
			tile(),
		},
		ui.column {
			grow = 1,
			min_w = 0,
			gap = "$space.1",
			ds.TextField {
				value = name,
				label = ui.t("shell.sidebar.project.edit.name"),
				placeholder = function(): string
					return ui.t("shell.sidebar.project.edit.name"):get()
				end,
				error = function(): string?
					return if unnamed:get() then ui.t("shell.sidebar.project.edit.name_empty"):get() else nil
				end,
				select_on_focus = true,
				w_full = true,
			},
			ui.text { where, text_size = "$font.size.xs", color = "$text.tertiary", truncate = true, literal = true, w_full = true },
		},
	}
end

-- The colours: a swatch each, the one shown ringed (its own, until one is
-- chosen; choosing its own again is choosing none).
local function colours(): Node
	local swatches: { Node } = {}
	for _, h in look.HUES do
		local function chosen(): boolean
			return shown_hue() == h
		end
		table.insert(
			swatches,
			-- (A ring round the colour, a gap between: two circles, one
			-- centre.)
			ui.button {
				label = ui.t(`shell.sidebar.project.edit.hue.{h}`),
				tooltip = locale.s(`shell.sidebar.project.edit.hue.{h}`),
				role = "radio",
				checked = chosen,
				w = RING,
				h = RING,
				px = 0,
				py = 0,
				shrink = 0,
				rounded = "$radius.full",
				items = "center",
				justify = "center",
				bg = "#00000000",
				border = 2,
				border_color = function(): string
					return if chosen() then "$text.primary" else "#00000000"
				end,
				cursor = "pointer",
				hover = { border_color = "$border" },
				focus = { outline = 1, outline_offset = 1, outline_color = "$focus.ring" },
				on_click = function()
					local id = editing:peek()
					hue:set(if id and look.hue(id) == h then nil else h)
				end,
				ui.div { w = SWATCH, h = SWATCH, rounded = "$radius.full", bg = `$palette.{h}.500` },
			}
		)
	end
	return ui.row {
		w_full = true,
		h = 28,
		items = "center",
		gap = "$space.3",
		ui.text { ui.t("shell.sidebar.project.edit.color"), text_size = "$font.size.sm", color = "$text.secondary", grow = 1 },
		ui.row { role = "radiogroup", label = ui.t("shell.sidebar.project.edit.color"), gap = "$space.1", items = "center", table.unpack(swatches) },
	}
end

-- Default: what it is (its repository's logo, or its initial), and its
-- colour.
local function default_panel(): Node
	return ui.column {
		w_full = true,
		gap = "$space.3",
		ui.text {
			function(): string
				local p = project()
				local art = if p then look.artwork(p) else nil
				return if art
					then ui.t("shell.sidebar.project.edit.default_artwork"):get()
					else ui.t("shell.sidebar.project.edit.default_initial"):get()
			end,
			text_size = "$font.size.sm",
			color = "$text.tertiary",
			wrap = true,
			w_full = true,
		},
		colours(),
	}
end

local function emoji_panel(): Node
	-- (The picker and its emoji, 180 KB of data, loaded the first time the
	-- panel shows, not with the window.)
	local EmojiPicker = require("@ds/emoji_picker")
	return ui.column {
		w_full = true,
		gap = "$space.3",
		EmojiPicker {
			id = "project-edit-emoji",
			value = emoji,
			recents = recents,
			tone = tone,
			columns = 10,
			height = 196,
			on_pick = function(e: string)
				emoji:set(e)
			end,
		},
		colours(),
	}
end

-- Image: nothing yet, a place to drop one (or choose); one chosen, the
-- stage to place it on; one already the project's, shown whole, to replace.
local function image_panel(): Node
	local from: { x: number, y: number, cx: number, cy: number }? = nil
	local stage_w = ui.state("project_edit_stage_w", 420)
	local function inset(): number
		return (stage_w:get() - VIEW) / 2
	end
	local placing = ui.div {
		id = "project-edit-crop",
		w_full = true,
		h = STAGE_H,
		relative = true,
		overflow = "hidden",
		rounded = "$radius.lg",
		bg = "$surface.sunken",
		cursor = "grab",
		role = "img",
		label = ui.t("shell.sidebar.project.edit.crop"),
		on_resize = function(size)
			stage_w:set(size.w)
		end,
		on_press = function(e: PointerEvent)
			local c = centre:peek()
			from = { x = e.x, y = e.y, cx = c.x, cy = c.y }
		end,
		on_pointer_move = function(e: PointerEvent)
			local f = from
			local px = picture:peek()
			if not f or not px then
				return
			end
			local k = VIEW * zoom:peek() / math.min(px.w, px.h)
			local x, y = placed(f.cx - (e.x - f.x) / (px.w * k), f.cy - (e.y - f.y) / (px.h * k))
			centre:set({ x = x, y = y })
		end,
		on_release = function()
			from = nil
		end,
		on_wheel = function(e: any): boolean
			zoom:set(math.clamp(zoom:peek() - (e.dy or 0) * 0.005, 1, 4))
			settle()
			return true
		end,
		-- The whole picture, dimmed (past the square, the stage clips it),
		-- and the square of it that will be the tile, clear, in the tile's
		-- own corner.
		ui.div {
			absolute = true,
			left = function(): number
				return inset()
			end,
			top = (STAGE_H - VIEW) / 2,
			w = VIEW,
			h = VIEW,
			opacity = 0.32,
			placed_picture(VIEW),
		},
		ui.div {
			absolute = true,
			left = function(): number
				return inset()
			end,
			top = (STAGE_H - VIEW) / 2,
			w = VIEW,
			h = VIEW,
			overflow = "hidden",
			rounded = ui.raw(corner(VIEW), "the tile's own corner, in proportion at the crop's size"),
			border = 1,
			border_color = "$border",
			placed_picture(VIEW),
		},
	}
	local placing_panel = ui.column {
		w_full = true,
		gap = "$space.3",
		placing,
		ui.row {
			w_full = true,
			items = "center",
			gap = "$space.3",
			ds.Icon { name = "gallery", size = 14 },
			ds.Slider { value = zoom, min = 1, max = 4, step = 0.01, label = ui.t("shell.sidebar.project.edit.zoom"), grow = 1, on_change = settle },
			ds.Button { ui.t("shell.sidebar.project.edit.replace") :: any, tone = "secondary", size = "sm", on_click = choose },
		},
	}
	local kept = ui.column {
		w_full = true,
		gap = "$space.3",
		ui.div {
			w_full = true,
			h = STAGE_H,
			rounded = "$radius.lg",
			bg = "$surface.sunken",
			items = "center",
			justify = "center",
			ui.div {
				w = VIEW,
				h = VIEW,
				overflow = "hidden",
				rounded = ui.raw(corner(VIEW), "the tile's own corner, in proportion at the crop's size"),
				ui.image {
					src = function(): string
						return look.file(image:get()) or ""
					end,
					w_full = true,
					h_full = true,
					fit = "cover",
				},
			},
		},
		ui.row {
			w_full = true,
			justify = "end",
			ds.Button { ui.t("shell.sidebar.project.edit.replace") :: any, tone = "secondary", size = "sm", on_click = choose },
		},
	}
	local empty = ui.column {
		id = "project-edit-drop",
		w_full = true,
		h = STAGE_H,
		rounded = "$radius.lg",
		bg = function(): string
			return if dropping:get() then "$hover" else "$surface.sunken"
		end,
		border = 1,
		border_color = function(): string
			return if dropping:get() then "$accent" else "$zeron.well.edge"
		end,
		items = "center",
		justify = "center",
		gap = "$space.2",
		ui.div { color = "$text.tertiary", pb = "$space.1", ds.Icon { name = "upload-minimalistic", size = 24 } },
		ui.text {
			function(): string
				return if dropping:get()
					then ui.t("shell.sidebar.project.edit.drop_release"):get()
					else ui.t("shell.sidebar.project.edit.drop"):get()
			end,
			text_size = "$font.size.md",
			color = "$text.secondary",
		},
		ui.text { ui.t("shell.sidebar.project.edit.formats"), text_size = "$font.size.xs", color = "$text.tertiary" },
		ui.div { pt = "$space.2", ds.Button { ui.t("shell.sidebar.project.edit.choose") :: any, tone = "secondary", size = "sm", on_click = choose } },
	}
	return ui.switch(function(): string
		return if picture:get() then "placing" elseif image:get() then "kept" else "empty"
	end, { placing = placing_panel, kept = kept, empty = empty })
end

-- What the project's icon becomes: nil for its default with its own colour.
local function wanted(): sessions.ProjectIcon?
	local k, h = kind:peek(), hue:peek()
	if k == "emoji" and emoji:peek() then
		return { emoji = emoji:peek(), hue = h }
	elseif k == "image" and image:peek() then
		return { image = image:peek(), hue = h }
	end
	return if h then { hue = h } else nil
end

local function same(a: sessions.ProjectIcon?, b: sessions.ProjectIcon?): boolean
	local x, y = a or {}, b or {}
	return x.emoji == y.emoji and x.image == y.image and x.hue == y.hue
end

local function save()
	if busy:peek() then
		return
	end
	local id = editing:peek()
	local p: sessions.Project? = nil
	for _, q in sessions.now:peek().projects do
		if q.id == id then
			p = q
		end
	end
	if not id or not p then
		open:set(false)
		return
	end
	local new_name = string.match(name:peek(), "^%s*(.-)%s*$") or ""
	if new_name == "" then
		unnamed:set(true)
		return
	end
	unnamed:set(false)
	failed:set(nil)
	busy:set(true)
	local before = p.icon
	local function done()
		busy:set(false)
		uploaded:set(nil)
		open:set(false)
		-- (An image of ours the project wore before, let go of.)
		if before and before.image and before.image ~= image:peek() then
			projects.forget_icon(before.image)
		end
	end
	-- (And the others it was opened with, those still there.)
	local targets: { sessions.Project } = { p :: sessions.Project }
	for _, other in also:peek() do
		for _, q in sessions.now:peek().projects do
			if q.id == other and q.id ~= id then
				table.insert(targets, q)
			end
		end
	end
	local function apply()
		for _, q in targets do
			if new_name ~= q.name then
				sessions.rename_project(q.id, new_name)
			end
		end
		local icon = wanted()
		local left, refused = 0, false
		for _, q in targets do
			if not same(icon, q.icon) then
				left += 1
			end
		end
		if left == 0 then
			done()
			return
		end
		for _, q in targets do
			if same(icon, q.icon) then
				continue
			end
			sessions.set_project_icon(q.id, icon):after(function()
				left -= 1
				if left == 0 and not refused then
					done()
				end
			end, function(err: any)
				if refused then
					return
				end
				refused = true
				busy:set(false)
				local word = if type(err) == "table" then err.message elseif type(err) == "string" then err else nil
				failed:set(if word == "outdated" or word == "gone" or word == "offline" then `save_{word}` else "save")
			end)
		end
	end
	-- A picture placed now: cut and kept first, then worn.
	if kind:peek() == "image" and picture:peek() then
		local px = picture:peek() :: Picture
		projects.upload_icon(px.path, crop()):after(function(url: any)
			image:set(url)
			uploaded:set(url)
			picture:set(nil)
			apply()
		end, function(err: any)
			busy:set(false)
			local word = if type(err) == "table" then err.message elseif type(err) == "string" then err else nil
			failed:set(if word == "offline" or word == "signed_out" or word == "too_large" or word == "unavailable" then word else "upload")
		end)
		return
	end
	apply()
end

function M.Dialog(): Node
	return ds.Dialog {
		open = open,
		title = ui.t("shell.sidebar.project.edit.title") :: any,
		size = "md",
		confirm = {
			label = function(): string
				return if busy:get()
					then ui.t("shell.sidebar.project.edit.saving"):get()
					else ui.t("shell.sidebar.project.edit.save"):get()
			end,
			keep_open = true,
			on_click = save,
		},
		ui.column {
			id = "project-edit",
			w_full = true,
			gap = "$space.4",
			-- A picture carried over any of it is taken (its head lit).
			on_file_hover = function(inside: boolean)
				dropping:set(inside)
			end,
			on_file_drop = dropped,
			head(),
			ds.Segmented {
				value = kind,
				label = ui.t("shell.sidebar.project.edit.icon"),
				fill = true,
				id = "project-edit-kind",
				options = {
					{ value = "default", label = ui.t("shell.sidebar.project.edit.kind.default") :: any },
					{ value = "emoji", label = ui.t("shell.sidebar.project.edit.kind.emoji") :: any },
					{ value = "image", label = ui.t("shell.sidebar.project.edit.kind.image") :: any },
				},
			},
			ui.switch(function(): string
				return kind:get()
			end, {
				default = default_panel(),
				emoji = emoji_panel(),
				image = image_panel(),
			}),
			ui.show(function(): boolean
				return failed:get() ~= nil
			end, ui.row {
				id = "project-edit-error",
				role = "alert",
				w_full = true,
				gap = "$space.2",
				items = "center",
				color = "$danger",
				ds.Icon { name = "danger-triangle", size = 14 },
				ui.text {
					function(): string
						local f = failed:get()
						return if f then ui.t(`shell.sidebar.project.edit.error.{f}`):get() else ""
					end,
					text_size = "$font.size.sm",
					wrap = true,
					grow = 1,
				},
			}),
		},
	}
end

return M
