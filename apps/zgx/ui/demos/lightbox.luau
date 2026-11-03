-- The lightbox: pictures opened from where they are, a message's
-- thumbnails and the composer's chips, and pictures of every shape and size
-- (a tiny icon, a tall one, a wide one, pixels to look at closely).

local Lightbox = require("@ds/lightbox")
local cargo = require("@ds/composer_cargo")
local kit = require("@demos/kit")

type Item = Lightbox.Item

-- A message's pictures, as its thumbnails show them.
local MESSAGE: { Item } = {
	{ src = "assets/composer/shell.png", name = "Screenshot 2026-10-30 at 11.42.07.png", w = 868, h = 617 },
	{ src = "assets/composer/transcript.png", name = "Screenshot 2026-10-30 at 11.43.52.png", w = 868, h = 617 },
	{ src = "assets/art/plate-1-night.jpg", name = "plate-1-night.jpg" },
}

-- Every shape and size.
local SHAPES: { Item } = {
	{ src = "assets/favicons/zeron.png", name = "zeron.png (64 × 64)" },
	{ src = "assets/lightbox/tall.png", name = "tall.png (520 × 1800)" },
	{ src = "assets/art/plate-2-bluehour.jpg", name = "plate-2-bluehour.jpg (2100 × 700)" },
	{ src = "assets/lightbox/pixels.png", name = "pixels.png (1200 × 800)" },
}

-- The composer's chips (ds/composer_cargo), opening the pictures among them.
local CARGO = ui.state("lb_cargo", {
	{ id = "lb-shell", kind = "image", name = "Screenshot 2026-10-30 at 11.42.07.png", src = "assets/composer/shell.png", meta = "PNG · 868 × 617" },
	{ id = "lb-dock", kind = "file", name = "composer_dock.rs", meta = "Rust · 48 KB" },
	{ id = "lb-art", kind = "image", name = "plate-3-sunrise.jpg", src = "assets/art/plate-3-sunrise.jpg", meta = "JPEG · 296 KB" },
} :: { cargo.Item })

local function cargo_images(): ({ Item }, { [string]: number })
	local out: { Item } = {}
	local at: { [string]: number } = {}
	for _, c in CARGO:get() do
		if c.kind == "image" and c.src then
			table.insert(out, { src = c.src, name = c.name, key = c.id })
			at[c.id] = #out
		end
	end
	return out, at
end

-- A message's thumbnail: Zeron's size and corner, a hairline over its edge.
local function thumb(group: string, items: { Item }, i: number, w: string, h: string, radius: string): Node
	local r = ui.theme.get(string.sub(radius, 2)) :: number
	return Lightbox.Thumb {
		group = group,
		index = i,
		items = items,
		radius = r,
		w = w,
		h = h,
		shrink = 0,
		rounded = radius,
		ui.overlay {
			w_full = true,
			h_full = true,
			rounded = radius,
			overflow = "hidden",
			border = 1,
			border_color = "$border",
			border_over = true,
			ui.image { src = items[i].src, fit = "cover", w_full = true, h_full = true },
		},
	}
end

local function strip(group: string, items: { Item }): Node
	local nodes: { Node } = {}
	for i = 1, #items do
		table.insert(nodes, thumb(group, items, i, "$zeron.prose.thumb.w", "$zeron.prose.thumb.h", "$zeron.prose.thumb.radius"))
	end
	return kit.row(table.unpack(nodes))
end

return {
	id = "lightbox",
	title = "Lightbox",
	about = "A picture opened from its thumbnail grows out of it into place, and goes back into it as it closes. Pinch or ⌘-scroll zooms about the pointer, a double click toggles fitted and its own size, a drag pans. ← → go between a set's pictures, ⌘C copies, Escape or a click outside closes. Its controls come up as the pointer moves.",
	demo = function()
		return kit.page(
			kit.block("A MESSAGE'S IMAGES", strip("lb-message", MESSAGE)),
			kit.block(
				"THE COMPOSER'S CHIPS",
				ui.div {
					w = 420,
					rounded = "$zeron.composer.radius",
					bg = "$zeron.composer.tint",
					pb = "$zeron.composer.chip.inset",
					cargo.Strip {
						items = CARGO,
						on_open = function(id: string)
							local images, at = cargo_images()
							if at[id] then
								Lightbox.open(images, at[id], "lb-cargo")
							end
						end,
						source = function(item: cargo.Item): { [string]: any }?
							return Lightbox.source("lb-cargo", item.id, ui.theme.get("zeron.composer.chip.radius") :: number)
						end,
					},
				}
			),
			kit.block("SHAPES AND SIZES", strip("lb-shapes", SHAPES)),
			Lightbox.Host()
		)
	end,
}
