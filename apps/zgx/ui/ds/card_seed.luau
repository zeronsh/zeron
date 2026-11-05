-- A member's card from a seed (their user id): which scene, which foil
-- finish, and where its character field starts, the same wherever the
-- card is drawn (here, on the web, on the server that renders its picture),
-- from one hash. Pure: no `ui`, nothing read, so a test can hold it to the
-- vectors the backend computes.
--
--   local card = CardSeed.pick("user_mara", 20)
--   card.scene   -- 19: the 20th scene of the manifest, in its `number` order
--   card.holo    -- "secret"
--   card.sky     -- 3389156178: offsets the character field
--
-- `scenes` is the count stored with the member when the card was made (21
-- today; 20 for anyone made before Black Hole), never the manifest's
-- length: scenes added later leave every card where it is.
--
-- The hash is FNV-1a over the string's UTF-8 bytes (offset 0x811c9dc5,
-- prime 0x01000193), finished with murmur3's fmix32. Every step is modulo
-- 2^32; the multiplies run in 16-bit halves, since a Luau number is a
-- double and a 32 by 32 bit product would lose its low bits.

local M = {}

-- The foil finishes, in the order the hash indexes them (member_card's
-- HOLOS: the same five, the same order).
export type Holo = "aurora" | "reverse" | "secret" | "starlight" | "glyph"
M.HOLOS = { "aurora", "reverse", "secret", "starlight", "glyph" } :: { Holo }

export type Card = { scene: number, holo: Holo, sky: number }

local TWO_16 = 0x10000
local TWO_32 = 0x100000000

-- `a * b` modulo 2^32, exactly: `a` split at its 16th bit, each half's
-- product under 2^48, well inside a double's 53 bits.
local function mul32(a: number, b: number): number
	local a_lo = a % TWO_16
	local a_hi = (a - a_lo) / TWO_16
	local lo = (a_lo * b) % TWO_32
	local hi = ((a_hi * b) % TWO_16) * TWO_16
	return (lo + hi) % TWO_32
end

local function fmix32(h: number): number
	h = bit32.bxor(h, bit32.rshift(h, 16))
	h = mul32(h, 0x85ebca6b)
	h = bit32.bxor(h, bit32.rshift(h, 13))
	h = mul32(h, 0xc2b2ae35)
	h = bit32.bxor(h, bit32.rshift(h, 16))
	return h
end

-- murmur3's finaliser, for others that need a well-mixed 32-bit number
-- from one (ds/private_text's stand-in letters).
M.mix = fmix32

-- The hash of a string: FNV-1a 32 over its bytes, then fmix32.
function M.hash(s: string): number
	local h = 0x811c9dc5
	for i = 1, #s do
		h = bit32.bxor(h, string.byte(s, i))
		h = mul32(h, 0x01000193)
	end
	return fmix32(h)
end

-- The card a seed makes, among `scenes` scenes.
function M.pick(seed: string, scenes: number): Card
	return {
		scene = M.hash(seed .. ":scene") % scenes,
		holo = M.HOLOS[M.hash(seed .. ":holo") % #M.HOLOS + 1],
		sky = M.hash(seed .. ":sky"),
	}
end

return M
