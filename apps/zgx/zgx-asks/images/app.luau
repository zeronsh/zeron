-- Repro: pictures as a lightbox shows them (Zeron's, apps/zgx/ui/ds/lightbox).
--
-- 1. A GIF in `ui.image`: a runtime error, not a picture (and, once it
--    decodes, a still one).
-- 2. `image_rendering = "pixelated"`: a 4 by 4 checker drawn at 64 by 64
--    should be 16 hard squares; it is drawn smooth.

local which = ui.state("which", "checker")

return ui.column {
	p = 16,
	gap = 16,
	ui.show(function(): boolean
		return which:get() == "gif"
	end, ui.image { id = "gif", src = "blink.gif", w = 64, h = 64 }),
	ui.div {
		id = "checker",
		w = 64,
		h = 64,
		ui.image { src = "checker.png", w = 64, h = 64, image_rendering = "pixelated", fit = "fill" },
	},
}
