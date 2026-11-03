--!strict
-- The scenes, each its folder under art/ and its layers back to front:
-- the first the sky (depth 0, still), the rest cut-outs at increasing
-- depth, moving more as they come nearer. (Zeron's member-card scenes,
-- the ones laid wide enough for a window.)

export type Scene = { slug: string, layers: { string } }

local SCENES: { Scene } = {
	{ slug = "violet-dusk", layers = { "sky", "far", "near" } },
	{ slug = "aurora", layers = { "sky", "far", "near" } },
	{ slug = "blossom", layers = { "sky", "far", "near", "detail" } },
	{ slug = "lighthouse", layers = { "sky", "far", "near" } },
	{ slug = "canyon", layers = { "back", "mid", "near", "detail" } },
	{ slug = "lotus", layers = { "back", "far", "near" } },
	{ slug = "icebergs", layers = { "back", "far", "near" } },
	{ slug = "wheat", layers = { "back", "far", "near" } },
	{ slug = "volcano", layers = { "back", "far", "near" } },
	{ slug = "terraces", layers = { "back", "far", "near" } },
	{ slug = "surf", layers = { "back", "far", "near" } },
	{ slug = "saturnine", layers = { "back", "far", "near", "detail" } },
	{ slug = "rooftops", layers = { "back", "far", "near" } },
	{ slug = "station", layers = { "back", "far", "near" } },
}

local M = {}

M.LIST = SCENES

-- A scene by its folder's name, or nil.
function M.find(slug: string): Scene?
	for _, s in SCENES do
		if s.slug == slug then
			return s
		end
	end
	return nil
end

-- One at random, not `but` while there are others.
function M.random(but: string?): Scene
	local pool: { Scene } = {}
	for _, s in SCENES do
		if s.slug ~= but then
			table.insert(pool, s)
		end
	end
	return pool[math.random(1, #pool)]
end

-- A layer's picture, from the plugin's folder.
function M.src(scene: Scene, layer: string): string
	return `art/{scene.slug}/{layer}.webp`
end

return M
