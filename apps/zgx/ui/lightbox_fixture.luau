-- The lightbox over a message's thumbnails and the composer's chips, for
-- tests/lightbox.test.luau. `test.lightbox_view` copies the open stage's
-- view (its scale and middle) into `test_lightbox_view`. Here, beside
-- app.luau, as a mounted script loads modules from its own directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Lightbox = require("@ds/lightbox")
local cargo = require("@ds/composer_cargo")

local MESSAGE: { Lightbox.Item } = {
	{ src = "assets/composer/shell.png", name = "shell.png" },
	{ src = "assets/composer/transcript.png", name = "transcript.png" },
	{ src = "assets/art/plate-1-night.jpg", name = "night.jpg" },
}
-- A picture smaller than any window.
local TINY: { Lightbox.Item } = { { src = "assets/favicons/zeron.png", name = "tiny.png" } }
-- A picture whose size was said wrong (a stale size kept for a path an agent
-- has since written over: 868 by 617 said as 400 by 400).
local STALE: { Lightbox.Item } = { { src = "assets/composer/shell.png", name = "stale.png", w = 400, h = 400 } }

local CARGO = ui.state("test_lightbox_cargo", {
	{ id = "c1", kind = "image", name = "chip-shell.png", src = "assets/composer/shell.png" },
	{ id = "c2", kind = "file", name = "notes.md" },
	{ id = "c3", kind = "image", name = "chip-tall.png", src = "assets/lightbox/tall.png" },
} :: { cargo.Item })

local view = ui.state("test_lightbox_view", nil :: any)
-- A test's host takes no pictures of the window: one stands in.
ui.command {
	id = "test.lightbox_snapshots",
	title = "Lightbox snapshots",
	run = function()
		Lightbox.snapshot = function(): string?
			return "assets/composer/shell.png"
		end
	end,
}
ui.command {
	id = "test.lightbox_view",
	title = "Lightbox view",
	run = function()
		view:set(Lightbox._view())
	end,
}

local thumbs: { Node } = {}
for i, it in MESSAGE do
	table.insert(thumbs, Lightbox.Thumb {
		group = "m1",
		index = i,
		items = MESSAGE,
		radius = 10,
		w = 120,
		h = 84,
		ui.image { src = it.src, fit = "cover", w_full = true, h_full = true },
	})
end

return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.5",
	gap = "$space.4",
	ui.button { label = "Before", "Before" },
	ui.row { gap = "$space.2", table.unpack(thumbs) },
	Lightbox.Thumb { group = "m2", index = 1, items = TINY, w = 40, h = 40, ui.image { src = TINY[1].src, w_full = true, h_full = true } },
	Lightbox.Thumb { group = "m3", index = 1, items = STALE, w = 120, h = 84, ui.image { src = STALE[1].src, fit = "cover", w_full = true, h_full = true } },
	cargo.Strip {
		items = CARGO,
		on_open = function(id: string)
			local images: { Lightbox.Item } = {}
			local at = 1
			for _, c in CARGO:peek() do
				if c.kind == "image" and c.src then
					table.insert(images, { src = c.src, name = c.name, key = c.id })
					if c.id == id then
						at = #images
					end
				end
			end
			Lightbox.open(images, at, "chips")
		end,
		source = function(item: cargo.Item): { [string]: any }?
			return Lightbox.source("chips", item.id, 10)
		end,
	},
	Lightbox.Host(),
	-- What's said for a moment, as the shell has it (ds/toast).
	ds.toast.Host {},
}
