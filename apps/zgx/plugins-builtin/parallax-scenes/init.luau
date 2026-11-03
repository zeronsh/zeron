--!strict
-- Parallax scenes: Zeron's member-card scenes behind a new chat, each
-- layer shifting against the pointer by its depth (the sky still, what is
-- nearest moving most), settling back as it leaves.
--
-- Built only from what any plugin can use. A new chat's background is a
-- place Zeron gives plugins (`#new-chat-background`): this replaces it
-- (`ui.change`), which makes it a choice in Settings → Appearance →
-- Wallpaper. Zeron fades what's drawn there and lays the chosen effect
-- (dither, ASCII, halftone, scanlines) over it; the place provides where
-- the pointer is (`pointer`, { x, y }, 0 to 1). Which scene, and how far
-- it moves, are this plugin's settings (its manifest's).

local zeron = require("./zeron")("parallax-scenes")
local scenes = require("./scenes")
local motion = require("@ds/motion")

local chosen = zeron.setting("scene")
local parallax = zeron.setting("parallax")

-- How far a layer moves at the pointer's edge, a step of depth: points
-- across and down, at Normal; and each setting's share of that.
local STEP_X, STEP_Y = 14, 9
local REACH = { off = 0, subtle = 0.5, normal = 1, deep = 1.8 } :: { [string]: number }
-- The springs a layer follows the pointer by (Zeron's).
local SPRING = { stiffness = 140, damping = 20 }

-- The last scene shuffled to: the next shuffle picks another.
local last: string? = nil

-- A scene, its layers laid over each other, each moving by its depth.
local function Scene(scene: scenes.Scene, reach: number, pointer: () -> any): Node
	local function axis(name: string): State<number>
		return ui.computed(function(): number
			local p = pointer()
			local v = if type(p) == "table" then p[name] else nil
			return if type(v) == "number" then v else 0.5
		end)
	end
	local px, py = axis("x"), axis("y")
	-- (Every layer laid in the same box, overhanging the place by as far
	-- as the nearest moves, so the layers meet as one picture at rest and
	-- no edge shows as they move.)
	local deepest = #scene.layers - 1
	local over_x, over_y = math.ceil(deepest * STEP_X * reach), math.ceil(deepest * STEP_Y * reach)
	local layers: { Node } = {}
	for i, layer in scene.layers do
		local d = (i - 1) * reach
		table.insert(
			layers,
			ui.div {
				absolute = true,
				left = -over_x,
				right = -over_x,
				top = -over_y,
				bottom = -over_y,
				x = if d > 0 then ui.spring(px, { from = d * STEP_X, to = -d * STEP_X, stiffness = SPRING.stiffness, damping = SPRING.damping }) else nil,
				y = if d > 0 then ui.spring(py, { from = d * STEP_Y, to = -d * STEP_Y, stiffness = SPRING.stiffness, damping = SPRING.damping }) else nil,
				ui.image { src = scenes.src(scene, layer), w_full = true, h_full = true, fit = "cover", aria_hidden = true },
			}
		)
	end
	return ui.div {
		id = "parallax-scene",
		class = `scene-{scene.slug}`,
		w_full = true,
		h_full = true,
		relative = true,
		overflow = "hidden",
		role = "img",
		label = zeron.s(`scene.{(string.gsub(scene.slug, "-", "_"))}`),
		table.unpack(layers),
	}
end

zeron.place {
	at = "#new-chat-background",
	fill = true,
	replace = function(): Node
		-- (Read here, in the build: where the pointer is over the page.)
		local pointer = zeron.live("pointer")
		-- Built again as a setting changes (a shuffle picks again each time
		-- a new chat's background is built: each new chat).
		return ui.presence(function(): string
			local reach = if motion.reduced() then 0 else (REACH[parallax:get()] or 1)
			return `{chosen:get()}|{reach}`
		end, function(key: string): Node
			local slug, reach = string.match(key, "^(.-)|(.*)$")
			local scene = scenes.find(slug or "")
			if slug == "shuffle" or not scene then
				scene = scenes.random(last)
			end
			local s = scene :: scenes.Scene
			last = s.slug
			return Scene(s, tonumber(reach) or 1, pointer)
		end, { w_full = true, h_full = true })
	end,
}
