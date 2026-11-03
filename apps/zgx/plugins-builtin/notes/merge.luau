--!strict
-- The merge of a chat's notes (crates/doc notes.rs, the engine's and the
-- shipping app's own rules), in bytes from 0 as an input's selection counts
-- them; edits never split a character. Pure: doc.luau's, and a test's
-- (src/plugins/notes_tests.rs).
--
--   merge.diff(old, new)            the smallest single replacement, or nil
--   merge.map_offset(edit, offset)  where an offset lands after it
--   merge.rebase(base, mine, remote)  both sides' typing kept
--   merge.hash(text)                crates/doc `notes_hash` (FNV-1a 64)
--   merge.supersedes(s, base)       whether a report came after the base

export type Snapshot = { text: string, hash: string, epoch: number, seq: number }
export type Edit = { from: number, to: number, insert: string }

local merge = {}

local function continuation(s: string, at: number): boolean
	-- (The byte after offset `at`, 0-based: a UTF-8 continuation byte.)
	local b = string.byte(s, at + 1)
	return b ~= nil and b >= 0x80 and b < 0xC0
end

-- The smallest single replacement turning `old` into `new`: bytes
-- `from`..`to` of `old` (0-based, `to` exclusive) become `insert`. Nil when
-- they're equal.
function merge.diff(old: string, new: string): Edit?
	if old == new then
		return nil
	end
	local o, n = #old, #new
	local prefix = 0
	local most = math.min(o, n)
	while prefix < most and string.byte(old, prefix + 1) == string.byte(new, prefix + 1) do
		prefix += 1
	end
	while prefix > 0 and (continuation(old, prefix) or continuation(new, prefix)) do
		prefix -= 1
	end
	local room = most - prefix
	local suffix = 0
	while suffix < room and string.byte(old, o - suffix) == string.byte(new, n - suffix) do
		suffix += 1
	end
	while suffix > 0 and (continuation(old, o - suffix) or continuation(new, n - suffix)) do
		suffix -= 1
	end
	return { from = prefix, to = o - suffix, insert = string.sub(new, prefix + 1, n - suffix) }
end

-- Where an offset into the old text lands in the new one: before the edit it
-- stays, after it it shifts, inside the replaced range it goes to the end of
-- what was put in.
function merge.map_offset(e: Edit, offset: number): number
	if offset <= e.from then
		return offset
	elseif offset >= e.to then
		return offset - (e.to - e.from) + #e.insert
	end
	return e.from + #e.insert
end

-- Three-way merge of two single-range edits of `base`: edits that don't
-- overlap both apply; overlapping ones honour both deletions and keep both
-- insertions, the remote one first. Nobody's typing is dropped.
function merge.rebase(base: string, mine: string, remote: string): string
	if mine == remote then
		return mine
	end
	local l, r = merge.diff(base, mine), merge.diff(base, remote)
	if not l then
		return remote
	elseif not r then
		return mine
	end
	local function cut(a: number, b: number): string
		return string.sub(base, a + 1, b)
	end
	if r.to <= l.from then
		return cut(0, r.from) .. r.insert .. cut(r.to, l.from) .. l.insert .. cut(l.to, #base)
	elseif l.to <= r.from then
		return cut(0, l.from) .. l.insert .. cut(l.to, r.from) .. r.insert .. cut(r.to, #base)
	end
	local from, to = math.min(l.from, r.from), math.max(l.to, r.to)
	return cut(0, from) .. r.insert .. l.insert .. cut(to, #base)
end

-- The notes' hash (crates/doc `notes_hash`): FNV-1a 64 over the bytes, 16
-- lowercase hex digits, in two 32-bit halves.
function merge.hash(text: string): string
	local hi, lo = 0xcbf29ce4, 0x84222325
	for i = 1, #text do
		lo = bit32.bxor(lo, string.byte(text, i))
		-- × 0x100000001b3: × 0x1b3, and the low half shifted 40 up (8 into
		-- the high half).
		local low = lo * 0x1b3
		local carry = math.floor(low / 0x100000000)
		hi = (hi * 0x1b3 + carry + bit32.lshift(lo, 8)) % 0x100000000
		lo = low % 0x100000000
	end
	return string.format("%08x%08x", hi, lo)
end

-- Whether `s` came after `base`: from another of the engine's doc
-- instances, or later from the same one.
function merge.supersedes(s: Snapshot, base: Snapshot): boolean
	return s.epoch ~= base.epoch or s.seq > base.seq
end

return merge
