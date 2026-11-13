-- Settings → Appearance: how Zeron looks on this Mac, the shell's own
-- look (screens/shell/appearance: the scheme, which setup chose too; a new
-- chat's wallpaper and its effect), the theme each scheme wears (a quick
-- pick; every theme is on the Themes page, screens/themes) and the window's
-- glass (ds.material).

local ds = require("@ds")
local material = require("@ds/material")
local math_ = require("@ds/math")
local appearance = require("@app/screens/shell/appearance")
local search = require("@app/screens/settings/search")
local prefs = require("@app/screens/settings/prefs")
local host = require("@app/screens/settings/host")
local wallpaper = require("@app/screens/settings/wallpaper")
local plugin_settings = require("@app/screens/plugins/settings")
local motion = require("@ds/motion")
local prefers = require("@ds/prefers")
local themes = require("@app/screens/themes/state")
local route = require("@app/screens/shell/route")

local M = {}

M.ID = "appearance"
M.TITLE = "Appearance"

search.index(M.ID, ui.t("settings.appearance.title"), ui.t("settings.appearance.terms"), {
	{ key = "scheme", title = ui.t("settings.appearance.scheme.label"), terms = ui.t("settings.appearance.scheme.terms") },
	{ key = "theme_light", title = ui.t("settings.appearance.theme.light"), terms = ui.t("settings.appearance.theme.terms") },
	{ key = "theme_dark", title = ui.t("settings.appearance.theme.dark"), terms = ui.t("settings.appearance.theme.terms") },
	{ key = "themes", title = ui.t("settings.appearance.theme.more"), terms = ui.t("settings.appearance.theme.terms") },
	{ key = "accent", title = ui.t("settings.appearance.accent.label"), terms = ui.t("settings.appearance.accent.terms") },
	{ key = "reduce_motion", title = ui.t("settings.appearance.reduce_motion.label"), section = ui.t("settings.appearance.motion"), terms = ui.t("settings.appearance.reduce_motion.terms") },
	{ key = "glass", title = ui.t("settings.appearance.glass.label"), terms = ui.t("settings.appearance.glass.terms") },
	{ key = "interface_font", title = ui.t("settings.appearance.interface_font.label"), section = ui.t("settings.appearance.fonts"), terms = ui.t("settings.appearance.interface_font.terms") },
	{ key = "text_size", title = ui.t("settings.appearance.text_size.label"), section = ui.t("settings.appearance.fonts"), terms = ui.t("settings.appearance.text_size.terms") },
	{ key = "code_font", title = ui.t("settings.appearance.code_font.label"), section = ui.t("settings.appearance.fonts"), terms = ui.t("settings.appearance.code_font.terms") },
	{ key = "code_font_size", title = ui.t("settings.appearance.code_font_size.label"), section = ui.t("settings.appearance.fonts"), terms = ui.t("settings.appearance.code_font_size.terms") },
	{ key = "conversation_width", title = ui.t("settings.appearance.conversation_width.label"), section = ui.t("settings.appearance.fonts"), terms = ui.t("settings.appearance.conversation_width.terms") },
	{ key = "render_math", title = ui.t("settings.appearance.render_math.label"), section = ui.t("settings.appearance.fonts"), terms = ui.t("settings.appearance.render_math.terms") },
	{ key = "math_font", title = ui.t("settings.appearance.math_font.label"), section = ui.t("settings.appearance.fonts"), terms = ui.t("settings.appearance.math_font.terms") },
	{ key = "math_size", title = ui.t("settings.appearance.math_size.label"), section = ui.t("settings.appearance.fonts"), terms = ui.t("settings.appearance.math_size.terms") },
	{ key = "wallpaper", title = ui.t("settings.appearance.wallpaper.label"), section = ui.t("settings.appearance.new_chat"), terms = ui.t("settings.appearance.wallpaper.terms") },
	{ key = "wallpaper_image", title = ui.t("settings.appearance.wallpaper.image.label"), section = ui.t("settings.appearance.new_chat"), terms = ui.t("settings.appearance.wallpaper.image.terms") },
	{ key = "effect", title = ui.t("settings.appearance.effect.label"), section = ui.t("settings.appearance.new_chat"), terms = ui.t("settings.appearance.effect.terms") },
})

local function row(key: string, props: any): Node
	return search.anchor(key, ds.Settings.Row(props))
end

-- A row that comes and goes in a card: its own hairline over it (a card's
-- rules are between rows that are there, never under the last).
local function later(on: () -> boolean, node: Node): Node
	return ui.show(on, ui.column { w_full = true, ds.Settings.Rule(), node })
end

-- The fonts installed on this computer (src/fonts.rs: `{ { family, mono }
-- }`, by name), asked for once a font picker opens; nil until they come.
local installed = ui.state("settings_installed_fonts", nil :: { { family: string, mono: boolean } }?)
local asked = false
local function ask_fonts()
	if asked then
		return
	end
	asked = true
	host.call("fonts.list"):after(function(list: any)
		installed:set(if type(list) == "table" then list else {})
	end, function()
		-- (Asked again as a picker next opens.)
		asked = false
		installed:set({})
	end)
end

-- A font picker (the interface's, or code's, which lists monospaced fonts
-- alone: a terminal's grid and code's columns need every letter one width):
-- the theme's own first ("Default (Geist)", its value ""), then this
-- computer's, searched by name. One chosen but no longer installed stays
-- listed, so the picker still says what's chosen (the theme's own draws).
local function font_picker(kind: "interface" | "code", value: State<string>, label: Message): Node
	local token = if kind == "code" then "font.mono" else "font.sans"
	local fallback = if kind == "code" then "Geist Mono" else "Geist"
	return ds.Dropdown {
		value = value :: any,
		label = label,
		width = 280,
		search = ui.t("settings.appearance.font_search"),
		on_open = ask_fonts,
		items = function(): { any }
			local chosen = value:get()
			local own = ui.theme.get(token)
			local out: { any } = {
				{ value = "", label = ui.t("settings.appearance.font_default", { name = if type(own) == "string" and own ~= "" then own else fallback }):get() },
			}
			local listed = chosen == ""
			for _, font in installed:get() or {} do
				if kind == "interface" or font.mono then
					table.insert(out, { value = font.family, label = font.family })
					listed = listed or font.family == chosen
				end
			end
			if not listed then
				table.insert(out, 2, { value = chosen, label = chosen })
			end
			return out
		end,
		empty = function(_query: string): any
			return { title = ui.t("settings.appearance.font_none") }
		end,
	}
end

-- Your picture, small, at the row's lead (nothing while there's none).
local THUMB = 36
local function thumbnail(): Node
	return ui.show(function(): boolean
		return appearance.image:get() ~= nil and not wallpaper.missing()
	end, ui.div {
		w = THUMB,
		h = THUMB,
		shrink = 0,
		rounded = "$radius.md",
		overflow = "hidden",
		border = 1,
		border_color = "$border",
		bg = "$zeron.secondary.bg",
		ui.presence(function(): string
			local image = appearance.image:get()
			return if image then image.path else ""
		end, function(path: string): Node
			local image = appearance.image:peek()
			if path == "" or not image then
				return ui.div {}
			end
			return ds.wallpaper.Picture { src = image.path, w = image.w, h = image.h, x = image.x, y = image.y, zoom = image.zoom }
		end, { w_full = true, h_full = true, relative = true }),
	})
end

-- Adjust: the picture as a new chat frames it, dragged to place and zoomed
-- (the slider, or the wheel), kept on Done. The frame has the window's
-- shape, as the shipping app's.
local FRAME_W, FRAME_H = 360, 225
local function adjust_dialog(): Node
	local from: { x: number, y: number, fx: number, fy: number }? = nil
	local zoom = ui.state("wallpaper_adjust_zoom", 1)
	-- (The slider's own state, kept with the draft both ways.)
	ui.effect(function()
		local z = wallpaper.draft:get().zoom
		if zoom:peek() ~= z then
			zoom:set(z)
		end
	end)
	local function set_zoom(z: number)
		local d = table.clone(wallpaper.draft:peek())
		d.zoom = math.clamp(z, ds.wallpaper.MIN_ZOOM, ds.wallpaper.MAX_ZOOM)
		wallpaper.draft:set(d)
	end
	local function picture(): any
		local image = appearance.image:get()
		local d = wallpaper.draft:get()
		if not image then
			return nil
		end
		return { src = image.path, w = image.w, h = image.h, x = d.x, y = d.y, zoom = d.zoom }
	end
	local function at(): { left: number, top: number, w: number, h: number }
		local p = picture()
		return if p then ds.wallpaper.cover(p, FRAME_W, FRAME_H) else { left = 0, top = 0, w = 0, h = 0 }
	end
	return ds.Dialog {
		open = wallpaper.adjusting,
		title = ui.t("settings.appearance.wallpaper.adjust.title"),
		body = ui.t("settings.appearance.wallpaper.adjust.body"),
		size = "md",
		confirm = { label = ui.t("settings.appearance.wallpaper.adjust.done"), on_click = wallpaper.keep_adjustment, keep_open = true },
		ui.column {
			w_full = true,
			items = "center",
			gap = "$space.4",
			ui.div {
				id = "wallpaper-adjust",
				w = FRAME_W,
				h = FRAME_H,
				shrink = 0,
				relative = true,
				rounded = "$radius.lg",
				overflow = "hidden",
				bg = "$zeron.secondary.bg",
				cursor = "grab",
				role = "img",
				label = ui.t("settings.appearance.wallpaper.adjust.preview"),
				on_press = function(e: PointerEvent)
					local d = wallpaper.draft:peek()
					from = { x = e.x, y = e.y, fx = d.x, fy = d.y }
				end,
				on_pointer_move = function(e: PointerEvent)
					local f = from
					if not f then
						return
					end
					-- (The overflow moves with the pointer: dragging right shows
					-- more of the left.)
					local a = at()
					local over_x, over_y = math.max(1, a.w - FRAME_W), math.max(1, a.h - FRAME_H)
					local d = table.clone(wallpaper.draft:peek())
					d.x = if a.w > FRAME_W then math.clamp(f.fx - (e.x - f.x) / over_x, 0, 1) else 0.5
					d.y = if a.h > FRAME_H then math.clamp(f.fy - (e.y - f.y) / over_y, 0, 1) else 0.5
					wallpaper.draft:set(d)
				end,
				on_release = function()
					from = nil
				end,
				on_wheel = function(e: any): boolean
					set_zoom(wallpaper.draft:peek().zoom - (e.dy or 0) * 0.005)
					return true
				end,
				ui.image {
					src = function(): string
						local p = picture()
						return if p then p.src else ""
					end,
					absolute = true,
					left = function(): number
						return at().left
					end,
					top = function(): number
						return at().top
					end,
					w = function(): number
						return at().w
					end,
					h = function(): number
						return at().h
					end,
					aria_hidden = true,
				},
			},
			ds.Slider {
				value = zoom,
				min = ds.wallpaper.MIN_ZOOM,
				max = ds.wallpaper.MAX_ZOOM,
				step = 0.01,
				label = ui.t("settings.appearance.wallpaper.adjust.zoom"),
				w = FRAME_W,
				on_change = function(v: number)
					set_zoom(v)
				end,
				format = function(v: number): string
					return `{math.floor(v * 100 + 0.5)}%`
				end,
			},
		},
	}
end

-- A scheme's theme, picked from every theme that has a variant for it,
-- grouped as the Themes page's tabs are. Its label says which scheme while
-- following the system ("Light theme"), else just "Theme".
local GROUPS = { builtin = "themes.page.builtin", template = "themes.page.from_templates", imported = "themes.page.yours" }
local function theme_row(kind: string): Node
	local chosen = ui.state(`settings_theme_{kind}`, themes.worn(kind))
	ui.effect(function()
		local now = themes.worn(kind)
		if chosen:peek() ~= now then
			chosen:set(now)
		end
	end)
	local function label(): string
		local key = if appearance.scheme:get() == "system" then kind else "one"
		return ui.t(`settings.appearance.theme.{key}`):get()
	end
	return row(`theme_{kind}`, {
		label = label,
		control = ds.Select {
			value = chosen,
			name = label,
			w = 220,
			on_change = function(id: string)
				themes.use_for(kind, id)
			end,
			options = function(): { any }
				local out = {}
				for _, e in themes.entries:get() do
					for _, id in e.variants do
						local p = themes.palette(id)
						if p and p.kind == kind then
							-- A built-in theme by its name (Gruvbox, not Gruvbox
							-- Dark); one of several variants of yours by both.
							local name = if e.source == "builtin" or #e.variants == 1 or p.name == e.name then e.name else `{e.name} {p.name}`
							table.insert(out, { value = id, label = name, group = ui.t(GROUPS[e.source] or GROUPS.builtin):get() })
						end
					end
				end
				return out
			end,
		},
	})
end

-- The quick picks: one for each scheme showing (both, following the
-- system), and the way to every theme.
local function theme_rows(): Node
	local scheme = appearance.scheme
	return ui.column {
		w_full = true,
		ui.show(function(): boolean
			return scheme:get() ~= "dark"
		end, theme_row("light")),
		ui.show(function(): boolean
			return scheme:get() == "system"
		end, ds.Settings.Rule()),
		ui.show(function(): boolean
			return scheme:get() ~= "light"
		end, theme_row("dark")),
		ds.Settings.Rule(),
		row("themes", {
			label = ui.t("settings.appearance.theme.more"),
			description = ui.t("settings.appearance.theme.more_description"),
			on_click = function()
				route.settings("themes")
			end,
		}),
	}
end

-- What the Wallpaper select shows: Appearance's choice, or nothing while a
-- plugin's wallpaper is used (picking one then takes it back).
local pick = ui.state("settings_wallpaper_pick", nil :: string?)
ui.effect(function()
	local now = if appearance.provider:get() then nil else appearance.wallpaper:get()
	if pick:peek() ~= now then
		pick:set(now)
	end
end)

function M.view(): Node
	return ds.Settings.Page {
		title = ui.t("settings.appearance.title"),
		description = ui.t("settings.appearance.description"),
		ds.Settings.Section {
			ds.Settings.Group {
				row("scheme", {
					label = ui.t("settings.appearance.scheme.label"),
					description = ui.t("settings.appearance.scheme.description"),
					control = ds.Segmented {
						value = appearance.scheme,
						label = ui.t("settings.appearance.scheme.label"),
						options = {
							{ value = "system", label = ui.t("settings.appearance.scheme.system") },
							{ value = "light", label = ui.t("settings.appearance.scheme.light") },
							{ value = "dark", label = ui.t("settings.appearance.scheme.dark") },
						} :: any,
					},
				}),
				theme_rows(),
				-- The accent: the theme's own, or one of Zeron's presets, the
				-- whole window wearing it (ds.look).
				row("accent", {
					label = ui.t("settings.appearance.accent.label"),
					description = function(): string
						return ds.look.find(ds.look.accent:get()).label
					end,
					control = ds.AppearancePicker.Accents { value = ds.look.accent },
				}),
				row("glass", {
					label = ui.t("settings.appearance.glass.label"),
					description = ui.t("settings.appearance.glass.description"),
					control = ds.Switch { value = material.frosted, label = ui.t("settings.appearance.glass.label") },
				}),
			},
		},
		-- Every move the app makes asks ds/motion: on, they land at once.
		ds.Settings.Section {
			title = ui.t("settings.appearance.motion"),
			ds.Settings.Group {
				row("reduce_motion", {
					label = ui.t("settings.appearance.reduce_motion.label"),
					description = function(): string
						if prefers.reduced_motion:get() then
							return ui.t("settings.appearance.reduce_motion.system_on"):get()
						end
						return ui.t("settings.appearance.reduce_motion.description"):get()
					end,
					control = ds.Switch { value = motion.reduce, label = ui.t("settings.appearance.reduce_motion.label") },
				}),
			},
		},
		ds.Settings.Section {
			title = ui.t("settings.appearance.fonts"),
			ds.Settings.Group {
				-- The interface's font and code's (ds.look), as the shipping
				-- app's Interface font and Code font: the window's font tokens,
				-- so everything set in them follows at once.
				row("interface_font", {
					label = ui.t("settings.appearance.interface_font.label"),
					description = ui.t("settings.appearance.interface_font.description"),
					control = font_picker("interface", ds.look.interface_font, ui.t("settings.appearance.interface_font.label")),
				}),
				-- The interface's text: menus, settings, conversations, every
				-- size and line height scaled from the body's (ds.look).
				row("text_size", {
					label = ui.t("settings.appearance.text_size.label"),
					description = ui.t("settings.appearance.text_size.description"),
					control = ds.Select {
						value = ds.look.text_size,
						name = ui.t("settings.appearance.text_size.label"),
						w = 132,
						-- (Read in the locale shown: they follow it.)
						options = function(): { any }
							local out = {}
							for _, n in ds.look.TEXT_SIZES do
								table.insert(out, { value = n, label = (if n == 13 then ui.t("settings.appearance.pt_default", { n = n }) else ui.t("settings.appearance.pt", { n = n })):get() })
							end
							return out
						end,
					},
				}),
				row("code_font", {
					label = ui.t("settings.appearance.code_font.label"),
					description = ui.t("settings.appearance.code_font.description"),
					control = font_picker("code", ds.look.code_font, ui.t("settings.appearance.code_font.label")),
				}),
				-- Code's size: the editor, markdown's code and diffs (not
				-- terminals), as the shipping app's Code & diff font.
				row("code_font_size", {
					label = ui.t("settings.appearance.code_font_size.label"),
					description = ui.t("settings.appearance.code_font_size.description"),
					control = ds.Select {
						value = prefs.code_font_size,
						name = ui.t("settings.appearance.code_font_size.label"),
						w = 112,
						options = function(): { any }
							local out = {}
							for _, n in prefs.CODE_FONT_SIZES do
								table.insert(out, { value = n, label = ui.t("settings.appearance.pt", { n = n }):get() })
							end
							return out
						end,
					},
				}),
				row("conversation_width", {
					label = ui.t("settings.appearance.conversation_width.label"),
					description = ui.t("settings.appearance.conversation_width.description"),
					control = ds.Slider {
						value = prefs.conversation_width,
						min = prefs.CONVERSATION_WIDTH.min,
						max = prefs.CONVERSATION_WIDTH.max,
						step = prefs.CONVERSATION_WIDTH.step,
						-- (The slider's name joins its value as a string: read as
						-- the page is built.)
						label = ui.t("settings.appearance.conversation_width.label"):get(),
						format = function(v: number): string
							return ui.t("settings.appearance.pt", { n = math.floor(v) }):get()
						end,
					},
				}),
				-- LaTeX in replies (ds/math), as the shipping app's "Render
				-- math", "Math font" and "Math size" (appearance.rs).
				row("render_math", {
					label = ui.t("settings.appearance.render_math.label"),
					description = ui.t("settings.appearance.render_math.description"),
					control = ds.Switch { value = math_.render, label = ui.t("settings.appearance.render_math.label") },
				}),
				row("math_font", {
					label = ui.t("settings.appearance.math_font.label"),
					control = ds.Select {
						value = math_.face,
						name = ui.t("settings.appearance.math_font.label"),
						w = 180,
						disabled = function(): boolean
							return not math_.render:get()
						end,
						options = (function(): { any }
							local out = {}
							for _, f in math_.FACES do
								table.insert(out, { value = f.value, label = f.label, description = f.style })
							end
							return out
						end)(),
					},
				}),
				row("math_size", {
					label = ui.t("settings.appearance.math_size.label"),
					description = ui.t("settings.appearance.math_size.description"),
					control = ds.Select {
						value = math_.size,
						name = ui.t("settings.appearance.math_size.label"),
						w = 112,
						disabled = function(): boolean
							return not math_.render:get()
						end,
						options = (function(): { any }
							local out = {}
							for _, n in math_.SIZES do
								table.insert(out, { value = n, label = `{n}%` })
							end
							return out
						end)(),
					},
				}),
			},
		},
		-- What a new chat opens on: nothing, your picture (the shipping
		-- app's one image, screens/settings/wallpaper), or what a plugin
		-- draws there; and Zeron's effect over it.
		ds.Settings.Section {
			title = ui.t("settings.appearance.new_chat"),
			description = ui.t("settings.appearance.new_chat_description"),
			-- (One card, its later rows bringing their own rules.)
			ui.column {
				id = "appearance-new-chat",
				w_full = true,
				ds.Settings.Group {
					ui.column {
						w_full = true,
						row("wallpaper", {
							label = ui.t("settings.appearance.wallpaper.label"),
							description = function(): string
								local p = appearance.provider:get()
								if p then
									return ui.t("settings.appearance.wallpaper.from_plugin", { name = p.name }):get()
								end
								local key = if appearance.wallpaper:get() == "image" then "image_description" else "none_description"
								return ui.t(`settings.appearance.wallpaper.{key}`):get()
							end,
							control = ui.row {
								gap = "$space.2",
								items = "center",
								-- (While a plugin's wallpaper is used: its settings, where
								-- it's chosen and let go.)
								ui.div {
									hidden = function(): boolean
										return appearance.provider:get() == nil
									end,
									ds.Button {
										function(): string
											local p = appearance.provider:get()
											return ui.t("settings.appearance.wallpaper.open_plugin", { name = if p then p.name else "" }):get()
										end,
										tone = "secondary",
										size = "sm",
										on_click = function()
											local p = appearance.provider:peek()
											if p then
												plugin_settings.open(p.id)
											end
										end,
									},
								},
								-- None or your picture: picking either takes the wallpaper
								-- back from a plugin. (Blank while a plugin's is used: the
								-- plugin's name stands in it.)
								ds.Select {
									value = pick,
									name = ui.t("settings.appearance.wallpaper.label"),
									placeholder = function(): string
										local p = appearance.provider:get()
										return if p then p.name else ""
									end,
									w = 180,
									on_change = appearance.choose,
									options = function(): { any }
										return {
											{ value = "none", label = ui.t("settings.appearance.wallpaper.none"):get() },
											{ value = "image", label = ui.t("settings.appearance.wallpaper.image.label"):get() },
										}
									end,
								},
							},
						}),
						-- Your picture: its name, and what can be done with it.
						later(function(): boolean
							return appearance.provider:get() == nil and appearance.wallpaper:get() == "image"
						end, row("wallpaper_image", {
							label = ui.t("settings.appearance.wallpaper.image.label"),
							lead = thumbnail(),
							description = function(): string
								local problem = wallpaper.problem_text()
								if problem ~= "" then
									return problem
								end
								local image = appearance.image:get()
								if not image then
									return ui.t("settings.appearance.wallpaper.image.none"):get()
								elseif wallpaper.missing() then
									return ui.t("settings.appearance.wallpaper.image.missing"):get()
								end
								return image.name
							end,
							control = ui.row {
								gap = "$space.2",
								items = "center",
								ui.div {
									hidden = function(): boolean
										return appearance.image:get() == nil or wallpaper.missing()
									end,
									ds.Button { ui.t("settings.appearance.wallpaper.image.adjust"), tone = "secondary", size = "sm", on_click = wallpaper.adjust },
								},
								ds.Button {
									function(): string
										local key = if appearance.image:get() then "replace" else "choose"
										return ui.t(`settings.appearance.wallpaper.image.{key}`):get()
									end,
									tone = "secondary",
									size = "sm",
									on_click = wallpaper.choose,
								},
								ui.div {
									hidden = function(): boolean
										return appearance.image:get() == nil
									end,
									ds.Button { ui.t("settings.appearance.wallpaper.image.remove"), tone = "secondary", size = "sm", on_click = wallpaper.remove },
								},
							},
						})),
						-- Zeron's effect over your picture (production's list, each
						-- saying what it does). A plugin's has its own, on its page.
						later(function(): boolean
							return appearance.provider:get() == nil and appearance.wallpaper:get() == "image"
						end, row("effect", {
							label = ui.t("settings.appearance.effect.label"),
							description = function(): string
								return ui.t(`settings.appearance.effect.{appearance.effect:get()}.description`):get()
							end,
							control = wallpaper.EffectSelect(appearance.effect),
						})),
					},
				},
			},
		},
		adjust_dialog(),
	}
end

return M
