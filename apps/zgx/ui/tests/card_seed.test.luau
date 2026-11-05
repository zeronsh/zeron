-- A member's card from a seed (ds/card_seed): the hash and the pick, held
-- to the backend's vectors (docs/card-vectors.json in the card-usernames
-- worktree, made by an independent Python implementation and matched by
-- edge/src/card.ts), copied in below as they are; every client must
-- reproduce them. Run with `zgx test ui`. The ids are made up.

local seed = require("@ds/card_seed")

-- The backend's vectors, as docs/card-vectors.json has them: 9 hashes
-- and 22 cards (`scene` from 0 into the scenes ordered by number;
-- `holo` from 0 into `holos`).
local HOLOS = { "aurora", "reverse", "secret", "starlight", "glyph" }
local HASHES = {
	{ input = "", hash = 2872998923 },
	{ input = "a", hash = 444641715 },
	{ input = "zeron", hash = 3920131753 },
	{ input = "user_test_0001:scene", hash = 62640894 },
	{ input = "user_test_0001:holo", hash = 1378625095 },
	{ input = "user_test_0001:sky", hash = 1678271501 },
	{ input = "é", hash = 2387039943 },
	{ input = "ユーザー", hash = 3445456616 },
	{ input = "🂡", hash = 3106548733 },
}
local CARDS = {
	{ id = "user_test_0001", scenes = 20, scene = 14, holo = 0, holoName = "aurora", sky = 1678271501 },
	{ id = "user_test_0002", scenes = 20, scene = 10, holo = 1, holoName = "reverse", sky = 2564363760 },
	{ id = "user_test_0003", scenes = 20, scene = 19, holo = 1, holoName = "reverse", sky = 2941292855 },
	{ id = "user_test_0004", scenes = 20, scene = 19, holo = 1, holoName = "reverse", sky = 2955699900 },
	{ id = "user_test_0005", scenes = 20, scene = 18, holo = 3, holoName = "starlight", sky = 1056250803 },
	{ id = "user_test_0006", scenes = 20, scene = 4, holo = 1, holoName = "reverse", sky = 3707144192 },
	{ id = "user_test_0007", scenes = 20, scene = 18, holo = 3, holoName = "starlight", sky = 1581408625 },
	{ id = "user_test_0008", scenes = 20, scene = 7, holo = 2, holoName = "secret", sky = 3955386535 },
	{ id = "user_test_0009", scenes = 20, scene = 17, holo = 2, holoName = "secret", sky = 1095194028 },
	{ id = "user_test_0010", scenes = 20, scene = 5, holo = 4, holoName = "glyph", sky = 2276114823 },
	{ id = "user_test_0011", scenes = 20, scene = 18, holo = 4, holoName = "glyph", sky = 1476704835 },
	{ id = "user_test_0012", scenes = 20, scene = 1, holo = 3, holoName = "starlight", sky = 269835148 },
	{ id = "user_01TESTTESTTESTTESTTESTTEST", scenes = 20, scene = 18, holo = 4, holoName = "glyph", sky = 2244616585 },
	{ id = "user_01FAKEFAKEFAKEFAKEFAKEFAKE", scenes = 20, scene = 14, holo = 1, holoName = "reverse", sky = 4065931191 },
	{ id = "a", scenes = 20, scene = 13, holo = 0, holoName = "aurora", sky = 3031281313 },
	{ id = "", scenes = 20, scene = 12, holo = 3, holoName = "starlight", sky = 3428309187 },
	{ id = "é", scenes = 20, scene = 3, holo = 2, holoName = "secret", sky = 610631180 },
	{ id = "ユーザー", scenes = 20, scene = 17, holo = 4, holoName = "glyph", sky = 1509394216 },
	{ id = "user_test_0001", scenes = 21, scene = 15, holo = 0, holoName = "aurora", sky = 1678271501 },
	{ id = "user_test_0002", scenes = 24, scene = 14, holo = 1, holoName = "reverse", sky = 2564363760 },
	{ id = "user_test_0003", scenes = 40, scene = 19, holo = 1, holoName = "reverse", sky = 2941292855 },
	{ id = "user_test_0004", scenes = 1, scene = 0, holo = 1, holoName = "reverse", sky = 2955699900 },
}

test("the hash matches the backend's, input by input", function()
	for _, h in HASHES do
		expect(seed.hash(h.input)).to_be(h.hash)
	end
end)

test("every one of the backend's cards comes out the same", function()
	for _, c in CARDS do
		local card = seed.pick(c.id, c.scenes)
		expect(card.scene).to_be(c.scene)
		expect(card.holo).to_be(c.holoName)
		expect(card.holo).to_be(HOLOS[c.holo + 1])
		expect(card.sky).to_be(c.sky)
	end
end)

test("the foil finishes are the backend's five, in its order", function()
	expect(seed.HOLOS).to_equal(HOLOS)
end)

test("the hash is FNV-1a over UTF-8 bytes, then fmix32, modulo 2^32", function()
	expect(seed.hash("")).to_be(2872998923)
	expect(seed.hash("a")).to_be(444641715)
	expect(seed.hash("foobar")).to_be(202221276)
	expect(seed.hash("mara")).to_be(787710844)
	-- Bytes, not code points: "ü" is two of them.
	expect(seed.hash("ü")).to_be(3244136902)
	expect(seed.hash("user_mara")).to_be(1556820978)
end)

test("the same seed makes the same card, and a different count a different scene", function()
	expect(seed.pick("user_mara", 20)).to_equal(seed.pick("user_mara", 20))
	-- Among 10 scenes, mara's hash lands elsewhere: the count is the
	-- member's, stored, so their card never moves as scenes are added.
	expect(seed.pick("mara", 10).scene).to_be(679750921 % 10)
	expect(seed.pick("mara", 20).scene).to_be(1)
	expect(seed.pick("mara", 20).holo).to_be("starlight")
end)
