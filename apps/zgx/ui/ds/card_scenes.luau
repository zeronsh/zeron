-- The card scenes, as assets/cards/manifest.json lists them, in its
-- `number` order: what a seed's scene index (ds/card_seed) points into. A
-- scene is its folder and its layers back to front, the first the sky
-- (depth 0), the rest cut-outs at increasing depth. A copy of the
-- manifest, since a script can't read it: when the manifest changes,
-- change this with it (and never reorder: a member's card is an index).

local M = {}

export type Scene = { slug: string, layers: { string }, depths: { number }? }

local SCENES: { Scene } = {
	{ slug = "violet-dusk", layers = { "sky", "far", "near" } },
	{ slug = "aurora", layers = { "sky", "far", "near" } },
	{ slug = "blossom", layers = { "sky", "far", "near", "detail" } },
	{ slug = "lighthouse", layers = { "sky", "far", "near" } },
	{ slug = "dunes", layers = { "sky", "detail", "far", "near" } },
	{ slug = "canyon", layers = { "back", "mid", "near", "detail" } },
	{ slug = "ravine", layers = { "back", "mid", "near", "detail" } },
	{ slug = "forest", layers = { "back", "mid", "near", "detail" } },
	{ slug = "bamboo", layers = { "back", "mid", "near", "detail" } },
	{ slug = "kelp", layers = { "back", "mid", "near", "detail" } },
	{ slug = "lotus", layers = { "back", "far", "near" } },
	{ slug = "icebergs", layers = { "back", "far", "near" } },
	{ slug = "wheat", layers = { "back", "far", "near" } },
	{ slug = "volcano", layers = { "back", "far", "near" } },
	{ slug = "salt-flats", layers = { "back", "far", "near" }, depths = { 0, 0.35, 1 } },
	{ slug = "terraces", layers = { "back", "far", "near" } },
	{ slug = "crystals", layers = { "back", "far", "near" } },
	{ slug = "surf", layers = { "back", "far", "near" } },
	{ slug = "mycelium", layers = { "back", "far", "near" } },
	{ slug = "saturnine", layers = { "back", "far", "near", "detail" } },
	{ slug = "black-hole", layers = { "back", "far", "near" } },
	{ slug = "ruins", layers = { "back", "far", "near" } },
	{ slug = "tram", layers = { "back", "far", "near" } },
	{ slug = "rooftops", layers = { "back", "far", "near" } },
	{ slug = "aqueduct", layers = { "back", "far", "near" } },
	{ slug = "station", layers = { "back", "far", "near" } },
}
M.SCENES = SCENES

-- How many there are today: what a new member's card is made among, and
-- is stored with them.
M.COUNT = #SCENES

-- A scene's layers as a card takes them (`layers`), by its index from a
-- seed (from 0, in manifest order). Past the end (a member made among
-- more scenes than this copy knows), the index wraps rather than failing.
function M.layers(index: number): { { src: string, depth: number } }
	local scene = SCENES[index % #SCENES + 1]
	local out = {}
	for i, name in scene.layers do
		table.insert(out, { src = `assets/cards/{scene.slug}/{name}.webp`, depth = if scene.depths then scene.depths[i] else i - 1 })
	end
	return out
end

-- Its folder's name, for a caption or a test.
function M.slug(index: number): string
	return SCENES[index % #SCENES + 1].slug
end

return M
