-- Inline atomic chips for the composer's textarea (`ui.textarea` `atoms`):
-- mentions (`@` files, chats, attachments; `/` commands; `$` skills) and
-- dragged cargo attachments live as atomic ranges in the draft. Deleting a
-- chip removes it in one Backspace without detaching its file from cargo;
-- deleting an attachment from cargo deletes every inline chip referencing
-- it; dragging a chip moves it in place; dragging a cargo chip drops an
-- inline chip at the pointer without duplicating the attachment.
--
-- Performance contract: every helper here is O(#atoms) or runs only on
-- explicit user actions (pick, drop, cargo remove, send). Nothing here
-- reads or copies the draft on ordinary keystrokes.

local FileIcon = require("@self/file_icons")
local Icon = require("@self/icon")

local M = {}

export type ChipData = {
	kind: string,
	label: string,
	link: string?,
	cargo_id: string?,
	mention: number?,
	icon: string?,
	file_icon: string?,
	dir: boolean?,
	trigger: string?,
	value: string?,
}

export type ChipSpec = {
	label: string,
	data: ChipData,
	style: { [string]: any },
	-- What assistive technology reads for it (a file's path).
	name: string?,
}

export type AtomEntry = {
	id: string,
	from: number,
	to: number,
	data: ChipData?,
	style: { [string]: any }?,
	name: string?,
}

local seq = 0
local function fresh_id(): string
	seq += 1
	return `chip-{seq}`
end

-- UTF-8 character count of `s`.
function M.char_len(s: string): number
	return (utf8.len(s) :: number?) or #s
end

-- 0-based character offset in `s` for 0-based byte offset `byte`.
function M.char_of_byte(s: string, byte: number): number
	local n = #s
	local b = math.clamp(byte, 0, n)
	if b <= 0 then
		return 0
	end
	while b > 0 and b < n do
		local c = string.byte(s, b + 1)
		if c < 0x80 or c >= 0xC0 then
			break
		end
		b -= 1
	end
	if b <= 0 then
		return 0
	end
	return (utf8.len(s, 1, b) :: number?) or b
end

-- 0-based byte offset in `s` for 0-based character offset `chars`.
function M.byte_of_char(s: string, chars: number): number
	if chars <= 0 then
		return 0
	end
	local off = utf8.offset(s, chars + 1)
	return if off then off - 1 else #s
end

local function icon_set(): string
	return if string.find(ui.theme.kind(), "dark", 1, true) then "dark" else "light"
end

local function ui_glyph_of(data: ChipData): string
	if data.icon and data.icon ~= "" then
		return data.icon
	end
	if data.kind == "chat" then
		return "side-chat"
	elseif data.kind == "image" then
		return "gallery"
	elseif data.kind == "annotation" then
		return "cursor"
	elseif data.kind == "skill" then
		return "magic-stick"
	elseif data.kind == "command" then
		return "command"
	end
	return "paperclip"
end

-- Atom style table for `ui.textarea`: matches transcript chips in
-- `ds/markdown.luau` (corner 5, `padding = { 2, 7, 2.4 }`, 17×17 `$zeron.chip.well`
-- with corner 3 and 2pt pad around the 13px icon, regular body weight).
local CHIP_PADDING = { 2, 7, 2.4 }
local CHIP_RADIUS = 5

function M.style_of(data: ChipData): { [string]: any }
	local is_action = data.kind == "command" or data.kind == "skill"
	local color = if is_action then "$zeron.composer.token.action" else "$text.primary"
	local icon: { [string]: any }
	if data.file_icon or data.kind == "file" then
		local fname = data.file_icon or data.label
		icon = {
			src = `assets/icons/files/{icon_set()}/{FileIcon.of(fname, data.dir == true)}.svg`,
			size = 13,
			gap = 2,
			bg = "$zeron.chip.well",
			radius = 3,
			pad = 2,
		}
	else
		icon = {
			src = `assets/icons/ui/{ui_glyph_of(data)}.svg`,
			size = 13,
			gap = 2,
			bg = "$zeron.chip.well",
			radius = 3,
			pad = 2,
			tint = if is_action then "currentColor" else "$zeron.chip.glyph",
		}
	end
	return {
		bg = "$zeron.chip.bg",
		color = color,
		padding = CHIP_PADDING,
		radius = CHIP_RADIUS,
		icon = icon,
	}
end

-- Snap byte offset `byte` out of any atom interior to the atom's nearest edge
-- so the drop caret never sits inside an atomic chip while dragging.
function M.snap_byte(text: string, atoms: { AtomEntry }, byte: number): number
	local n = #text
	local at = math.clamp(byte, 0, n)
	if #atoms == 0 then
		return at
	end
	for _, a in atoms do
		local b_from = M.byte_of_char(text, a.from)
		local b_to = M.byte_of_char(text, a.to)
		if at > b_from and at < b_to then
			return if (at - b_from) <= (b_to - at) then b_from else b_to
		end
	end
	return at
end

-- Compact pill preview rendered under the pointer while dragging a chip:
-- mirrors the real inline chip anatomy (22px tall, 17×17 icon well, corner 5).
function M.chip_preview(data: ChipData?): Node
	local d: ChipData = data or { kind = "attachment", label = "Item" }
	local is_action = d.kind == "command" or d.kind == "skill"
	local ink = if is_action then "$zeron.composer.token.action" else "$text.primary"
	local lead: Node
	if d.file_icon or d.kind == "file" then
		lead = FileIcon { name = d.file_icon or d.label, dir = d.dir == true, size = 13 }
	else
		lead = Icon {
			name = ui_glyph_of(d) :: any,
			size = 13,
			color = if is_action then "$zeron.composer.token.action" else "$zeron.chip.glyph",
		}
	end
	return ui.row {
		h = ui.raw(22, "inline chip drag preview height"),
		pl = ui.raw(2, "inline chip drag preview left inset"),
		pr = ui.raw(7, "inline chip drag preview right inset"),
		gap = ui.raw(2, "inline chip drag preview icon gap"),
		items = "center",
		rounded = ui.raw(5, "inline chip drag preview corner"),
		bg = "$zeron.chip.bg",
		border = 1,
		border_color = "$zeron.composer.edge",
		shadow = "$zeron.composer.shadow",
		opacity = 0.94,
		pointer_events = "none",
		ui.div {
			w = ui.raw(17, "inline chip drag preview icon well width"),
			h = ui.raw(17, "inline chip drag preview icon well height"),
			shrink = 0,
			items = "center",
			justify = "center",
			rounded = ui.raw(3, "inline chip drag preview icon well corner"),
			bg = "$zeron.chip.well",
			lead,
		},
		ui.text {
			d.label,
			literal = true,
			text_size = "$zeron.composer.text",
			line_height = "$zeron.composer.line",
			color = ink,
		},
	}
end

-- Builds a chip spec from a completion item picked from `@`, `/`, or `$`.
-- Returns nil for built-in Zeron commands (`item.zeron` / `item.consume`)
-- which execute directly rather than becoming chips.
function M.from_mention(item: any, trigger: string, token: string): ChipSpec?
	if type(item) ~= "table" or item.zeron or item.consume then
		return nil
	end
	local data: ChipData
	if trigger == "@" then
		if item.section == "attachments" then
			local what = item.attachment_kind or "file"
			local kind = if what == "image" or what == "appshot"
				then "image"
				elseif what == "chat" then "chat"
				elseif what == "annotation" then "annotation"
				else "attachment"
			local label = item.chip_label or item.label
			data = {
				kind = kind,
				label = label,
				link = item.link,
				cargo_id = item.cargo_id,
				mention = item.mention,
				icon = item.icon,
				file_icon = item.file_icon,
				trigger = "@",
				value = item.value,
			}
		elseif item.section == "chats" then
			data = {
				kind = "chat",
				label = item.label,
				link = item.link,
				icon = "side-chat",
				trigger = "@",
				value = item.value,
			}
		else
			-- File or folder in the project: display its shortest unique name.
			local label = if string.sub(token, 1, 1) == "@" then string.sub(token, 2) else item.label
			data = {
				kind = "file",
				label = label,
				link = item.link or (if type(item.value) == "string" and item.value ~= "" then "@" .. item.value else token),
				file_icon = item.label,
				dir = item.dir == true,
				trigger = "@",
				value = item.value,
			}
		end
	else
		local is_skill = trigger == "$" or item.icon == "magic-stick"
		data = {
			kind = if is_skill then "skill" else "command",
			label = token,
			link = item.link,
			icon = item.icon or (if is_skill then "magic-stick" else "command"),
			trigger = trigger,
			value = item.value,
		}
	end
	if type(data.label) ~= "string" or data.label == "" then
		return nil
	end
	return {
		label = data.label,
		data = data,
		style = M.style_of(data),
		-- (Read aloud by its whole name: a file by its path, as the chip
		-- shows the shortest end of it.)
		name = if data.kind == "file" and type(item.value) == "string" and item.value ~= "" then item.value else nil,
	}
end

-- Builds a chip spec from a cargo item (when dragged from the cargo strip
-- into the composer). `link`, `label`, `kind`, and `icon` are supplied by
-- the caller (typically via `Carried.mention` / `Carried.label` / `Carried.kind`).
function M.from_cargo(item: any, label: string, link: string, what: string, icon: string?): ChipSpec?
	if type(item) ~= "table" or label == "" or link == "" then
		return nil
	end
	local kind = if what == "image" or what == "appshot"
		then "image"
		elseif what == "chat" then "chat"
		elseif what == "annotation" then "annotation"
		else "attachment"
	local data: ChipData = {
		kind = kind,
		label = label,
		link = link,
		cargo_id = item.id,
		mention = item.mention,
		icon = icon,
		file_icon = if what == "file" then item.name else nil,
	}
	return {
		label = label,
		data = data,
		style = M.style_of(data),
	}
end

-- Returns true if the byte range `[byte_from, byte_to)` overlaps or touches
-- the end of an existing atom in `atoms`.
function M.touches_atom(text: string, atoms: { AtomEntry }?, byte_from: number, byte_to: number): boolean
	if not atoms or #atoms == 0 then
		return false
	end
	local c0 = M.char_of_byte(text, byte_from)
	local c1 = M.char_of_byte(text, byte_to)
	for _, a in atoms do
		if a.from < c1 and a.to > c0 then
			return true
		end
		if c0 == a.to and c1 == a.to then
			return true
		end
	end
	return false
end

local function clone_atom(a: AtomEntry, from: number, to: number): AtomEntry
	return {
		id = a.id,
		from = from,
		to = to,
		data = a.data,
		style = a.style,
		name = a.name,
	}
end

local function sort_atoms(list: { AtomEntry })
	table.sort(list, function(a: AtomEntry, b: AtomEntry): boolean
		return a.from < b.from
	end)
end

-- Pure splice: replaces `[byte_from, byte_to)` in `text` with `spec` (with
-- clean spacing) and returns `(next_text, next_atoms, caret_byte, inserted)`.
function M.splice_insert(
	text: string,
	atoms: { AtomEntry }?,
	byte_from: number,
	byte_to: number,
	spec: ChipSpec,
	add_space: boolean?
): (string, { AtomEntry }, number, string)
	local n = #text
	local b0 = math.clamp(byte_from, 0, n)
	local b1 = math.clamp(byte_to, b0, n)

	-- Snap a point insertion out of any existing atom so dropping on a chip
	-- never splits it.
	local cur: { AtomEntry } = atoms or {}
	if b0 == b1 and #cur > 0 then
		for _, a in cur do
			local ab0 = M.byte_of_char(text, a.from)
			local ab1 = M.byte_of_char(text, a.to)
			if b0 > ab0 and b0 < ab1 then
				b0 = if (b0 - ab0) < (ab1 - b0) then ab0 else ab1
				b1 = b0
				break
			end
		end
	end

	local lead = ""
	if b0 == b1 and b0 > 0 then
		local prev = string.sub(text, b0, b0)
		if not string.match(prev, "[%s%(%[{>]") then
			lead = " "
		end
	end
	local trail = ""
	if add_space ~= false then
		local after = string.sub(text, b1 + 1, b1 + 1)
		if after == "" or not string.match(after, "%s") then
			trail = " "
		end
	end

	local inserted = lead .. spec.label .. trail
	local next_text = string.sub(text, 1, b0) .. inserted .. string.sub(text, b1 + 1)

	local del_c0 = M.char_of_byte(text, b0)
	local del_c1 = M.char_of_byte(text, b1)
	local ins_chars = M.char_len(inserted)
	local delta = ins_chars - (del_c1 - del_c0)

	local next_atoms: { AtomEntry } = {}
	for _, a in cur do
		if a.to <= del_c0 then
			table.insert(next_atoms, clone_atom(a, a.from, a.to))
		elseif a.from >= del_c1 then
			table.insert(next_atoms, clone_atom(a, a.from + delta, a.to + delta))
		end
	end
	local chip_c0 = M.char_of_byte(next_text, b0 + #lead)
	local chip_c1 = M.char_of_byte(next_text, b0 + #lead + #spec.label)
	table.insert(next_atoms, {
		id = fresh_id(),
		from = chip_c0,
		to = chip_c1,
		data = spec.data,
		style = spec.style,
		name = spec.name,
	})
	sort_atoms(next_atoms)
	return next_text, next_atoms, b0 + #inserted, inserted
end

-- Inserts `spec` into the draft (and mounted editor `el`, if any) replacing
-- `[byte_from, byte_to)`. Returns the new caret byte offset.
function M.insert_at(
	el: any,
	value_state: State<string>,
	atoms_state: State<{ AtomEntry }>,
	byte_from: number,
	byte_to: number,
	spec: ChipSpec,
	add_space: boolean?
): number
	local text = value_state:peek()
	local cur = M.live(atoms_state, el)
	local next_text, next_atoms, caret_byte, inserted = M.splice_insert(text, cur, byte_from, byte_to, spec, add_space)
	if el then
		local b0 = math.clamp(byte_from, 0, #text)
		local b1 = math.clamp(byte_to, b0, #text)
		-- Snap point insertion consistently with splice_insert.
		if b0 == b1 and #cur > 0 then
			for _, a in cur do
				local ab0 = M.byte_of_char(text, a.from)
				local ab1 = M.byte_of_char(text, a.to)
				if b0 > ab0 and b0 < ab1 then
					b0 = if (b0 - ab0) < (ab1 - b0) then ab0 else ab1
					b1 = b0
					break
				end
			end
		end
		el:set_selection_bytes(b0, b1)
		el:replace_selection(inserted)
		atoms_state:set(next_atoms)
	else
		M.apply(value_state, atoms_state, next_text, next_atoms)
	end
	return caret_byte
end

-- Pure move: moves atom `atom_id` to `target_byte`, cleaning up redundant
-- whitespace at its old home and keeping clean spacing at its destination.
-- Returns `(next_text, next_atoms, caret_byte, moved)`.
function M.splice_move(
	text: string,
	atoms: { AtomEntry }?,
	atom_id: string,
	target_byte: number
): (string, { AtomEntry }, number, boolean)
	local cur: { AtomEntry } = atoms or {}
	local src: AtomEntry? = nil
	for _, a in cur do
		if a.id == atom_id then
			src = a
			break
		end
	end
	if not src then
		return text, cur, target_byte, false
	end

	local src_b0 = M.byte_of_char(text, src.from)
	local src_b1 = M.byte_of_char(text, src.to)
	if src_b0 >= src_b1 or src_b1 > #text then
		return text, cur, target_byte, false
	end

	local dst = math.clamp(target_byte, 0, #text)
	-- Dropped onto itself: no-op.
	if dst >= src_b0 and dst <= src_b1 then
		return text, cur, src_b1, false
	end

	-- Snap destination out of any other atom so we never split a chip.
	for _, a in cur do
		if a.id ~= atom_id then
			local ab0 = M.byte_of_char(text, a.from)
			local ab1 = M.byte_of_char(text, a.to)
			if dst > ab0 and dst < ab1 then
				dst = if (dst - ab0) < (ab1 - dst) then ab0 else ab1
				break
			end
		end
	end

	local label = string.sub(text, src_b0 + 1, src_b1)

	-- Step 1: remove the chip from its old position, collapsing any double
	-- space or orphan edge space it leaves behind.
	local rm_b0, rm_b1 = src_b0, src_b1
	local before = if rm_b0 > 0 then string.sub(text, rm_b0, rm_b0) else ""
	local after = if rm_b1 < #text then string.sub(text, rm_b1 + 1, rm_b1 + 1) else ""
	if before == " " and after == " " then
		rm_b1 += 1
	elseif rm_b0 == 0 and after == " " then
		rm_b1 += 1
	elseif rm_b1 == #text and before == " " then
		rm_b0 -= 1
	end

	local rm_c0 = M.char_of_byte(text, rm_b0)
	local rm_c1 = M.char_of_byte(text, rm_b1)
	local rm_chars = rm_c1 - rm_c0
	local rm_bytes = rm_b1 - rm_b0
	local mid_text = string.sub(text, 1, rm_b0) .. string.sub(text, rm_b1 + 1)

	if dst > rm_b1 then
		dst -= rm_bytes
	elseif dst > rm_b0 then
		dst = rm_b0
	end
	dst = math.clamp(dst, 0, #mid_text)

	local mid_atoms: { AtomEntry } = {}
	for _, a in cur do
		if a.id ~= atom_id then
			if a.from >= rm_c1 then
				table.insert(mid_atoms, clone_atom(a, a.from - rm_chars, a.to - rm_chars))
			else
				table.insert(mid_atoms, clone_atom(a, a.from, a.to))
			end
		end
	end

	-- Step 2: insert the chip at `dst` in `mid_text` with clean separation.
	local lead = ""
	if dst > 0 then
		local prev = string.sub(mid_text, dst, dst)
		if not string.match(prev, "[%s%(%[{>]") then
			lead = " "
		end
	end
	local trail = ""
	local next_ch = string.sub(mid_text, dst + 1, dst + 1)
	if next_ch == "" or not string.match(next_ch, "[%s%.,;:!%?)%]%}]") then
		trail = " "
	end

	local inserted = lead .. label .. trail
	local next_text = string.sub(mid_text, 1, dst) .. inserted .. string.sub(mid_text, dst + 1)
	local ins_c = M.char_of_byte(mid_text, dst)
	local ins_chars = M.char_len(inserted)

	local next_atoms: { AtomEntry } = {}
	for _, a in mid_atoms do
		if a.from >= ins_c then
			table.insert(next_atoms, clone_atom(a, a.from + ins_chars, a.to + ins_chars))
		else
			table.insert(next_atoms, a)
		end
	end
	local chip_c0 = M.char_of_byte(next_text, dst + #lead)
	local chip_c1 = M.char_of_byte(next_text, dst + #lead + #label)
	table.insert(next_atoms, {
		id = src.id,
		from = chip_c0,
		to = chip_c1,
		data = src.data,
		style = src.style,
		name = src.name,
	})
	sort_atoms(next_atoms)
	return next_text, next_atoms, dst + #inserted, true
end

-- Mounted `ui.textarea` refs keyed weakly by their `atoms` state so
-- programmatic draft + atoms updates (e.g. removing an attachment from the
-- cargo strip, drag-moving a chip, or switching chats) update the editor
-- directly before writing `atoms_state` — avoiding `set_text` clearing atoms.
local bound_editors: { [any]: any } = setmetatable({}, { __mode = "k" }) :: any

function M.bind_editor(atoms_state: State<{ AtomEntry }>, el_ref: any)
	bound_editors[atoms_state] = el_ref
end

local function bound_target(atoms_state: State<{ AtomEntry }>, el: any?): any
	if el then
		return el
	end
	local ref = bound_editors[atoms_state]
	return if ref then ref:peek() else nil
end

-- Returns the authoritative current atoms for `atoms_state`: reads directly
-- from the mounted editor when available (so native backspace/edit/undo shifts
-- are always exact) and falls back to `atoms_state:peek()` when unmounted.
function M.live(atoms_state: State<{ AtomEntry }>, el: any?): { AtomEntry }
	local target = bound_target(atoms_state, el)
	if target then
		local live = ui.editor.atoms(target)
		if type(live) == "table" then
			return live
		end
	end
	return atoms_state:peek() or {}
end

-- Atomically updates `value_state` and `atoms_state` to `(next_text, next_atoms)`.
-- When a mounted editor is bound to `atoms_state` (or passed as `el`), replaces
-- the editor's buffer in-place so `ui.textarea`'s `value` effect doesn't fire
-- `set_text` and wipe `next_atoms`.
function M.apply(
	value_state: State<string>,
	atoms_state: State<{ AtomEntry }>,
	next_text: string,
	next_atoms: { AtomEntry },
	el: any?,
	caret_byte: number?
)
	local target = bound_target(atoms_state, el)
	local cur_text = value_state:peek()
	if target and next_text ~= cur_text then
		local prev_a = nil :: number?
		if caret_byte == nil then
			local a = target:selection_bytes()
			if type(a) == "number" then
				prev_a = a
			end
		end
		target:set_selection_bytes(0, #cur_text)
		target:replace_selection(next_text)
		if value_state:peek() == next_text then
			atoms_state:set(next_atoms)
			if caret_byte ~= nil then
				local c = math.clamp(caret_byte, 0, #next_text)
				target:set_selection_bytes(c, c)
			elseif prev_a ~= nil then
				local c = math.clamp(prev_a, 0, #next_text)
				target:set_selection_bytes(c, c)
			end
			return
		end
	end
	if value_state:peek() ~= next_text then
		value_state:set(next_text)
	end
	atoms_state:set(next_atoms)
	if target and #next_atoms > 0 then
		-- Fallback for read-only editors where `replace_selection` is ignored
		-- and `value_state:set` triggers `set_text` on effect flush.
		ui.after(0, function()
			if value_state:peek() == next_text and #(M.live(atoms_state, target)) == 0 then
				atoms_state:set(next_atoms)
			end
		end)
	end
end

-- Moves inline atom `atom_id` to `target_byte` in the composer.
function M.move_atom(
	el: any,
	value_state: State<string>,
	atoms_state: State<{ AtomEntry }>,
	atom_id: string,
	target_byte: number
): boolean
	local text = value_state:peek()
	local cur = M.live(atoms_state, el)
	local next_text, next_atoms, caret_byte, moved = M.splice_move(text, cur, atom_id, target_byte)
	if not moved then
		return false
	end
	M.apply(value_state, atoms_state, next_text, next_atoms, el, caret_byte)
	if el then
		el:focus()
		el:set_selection_bytes(caret_byte, caret_byte)
	end
	return true
end

-- Pure removal: deletes every atom whose `data.cargo_id == cargo_id` from
-- `text` and `atoms` (right-to-left, collapsing redundant spaces).
-- Returns `(next_text, next_atoms, changed)`.
function M.remove_by_cargo(text: string, atoms: { AtomEntry }?, cargo_id: string): (string, { AtomEntry }, boolean)
	if not atoms or #atoms == 0 then
		return text, atoms or {}, false
	end
	local victim: { AtomEntry } = {}
	local kept: { AtomEntry } = {}
	for _, a in atoms do
		if a.data and a.data.cargo_id == cargo_id then
			table.insert(victim, a)
		else
			table.insert(kept, clone_atom(a, a.from, a.to))
		end
	end
	if #victim == 0 then
		return text, atoms, false
	end
	-- Process right-to-left so earlier byte/char offsets stay valid.
	table.sort(victim, function(a: AtomEntry, b: AtomEntry): boolean
		return a.from > b.from
	end)
	local out = text
	for _, v in victim do
		local b0 = M.byte_of_char(out, v.from)
		local b1 = M.byte_of_char(out, v.to)
		if b0 < b1 and b1 <= #out then
			local before = if b0 > 0 then string.sub(out, b0, b0) else ""
			local after = if b1 < #out then string.sub(out, b1 + 1, b1 + 1) else ""
			if before == " " and after == " " then
				b1 += 1
			elseif b0 == 0 and after == " " then
				b1 += 1
			elseif b1 == #out and before == " " then
				b0 -= 1
			end
			local rm_c0 = M.char_of_byte(out, b0)
			local rm_c1 = M.char_of_byte(out, b1)
			local rm_chars = rm_c1 - rm_c0
			out = string.sub(out, 1, b0) .. string.sub(out, b1 + 1)
			for _, k in kept do
				if k.from >= rm_c1 then
					k.from -= rm_chars
					k.to -= rm_chars
				end
			end
		end
	end
	sort_atoms(kept)
	return out, kept, true
end

-- Replaces each atom's `[from, to)` range with its canonical `data.link`
-- (right-to-left) for sending to the agent.
function M.encode(text: string, atoms: { AtomEntry }?): string
	if not atoms or #atoms == 0 or text == "" then
		return text
	end
	local linked: { AtomEntry } = {}
	for _, a in atoms do
		if a.data and type(a.data.link) == "string" and a.data.link ~= "" then
			table.insert(linked, a)
		end
	end
	if #linked == 0 then
		return text
	end
	table.sort(linked, function(a: AtomEntry, b: AtomEntry): boolean
		return a.from > b.from
	end)
	local out = text
	for _, a in linked do
		local b0 = M.byte_of_char(out, a.from)
		local b1 = M.byte_of_char(out, a.to)
		if b0 < b1 and b1 <= #out then
			out = string.sub(out, 1, b0) .. (a.data :: ChipData).link :: string .. string.sub(out, b1 + 1)
		end
	end
	return out
end

-- Refreshes every atom's theme-dependent style (e.g. dark/light file icons).
function M.restyle(atoms: { AtomEntry }?): { AtomEntry }
	if not atoms or #atoms == 0 then
		return atoms or {}
	end
	local out: { AtomEntry } = {}
	for _, a in atoms do
		table.insert(out, {
			id = a.id,
			from = a.from,
			to = a.to,
			data = a.data,
			style = if a.data then M.style_of(a.data) else a.style,
			name = a.name,
		})
	end
	return out
end

-- Shifts all atom ranges forward by `char_offset` (used when appending a
-- lifted draft onto existing text).
function M.shift(atoms: { AtomEntry }?, char_offset: number): { AtomEntry }
	if not atoms or #atoms == 0 then
		return {}
	end
	local out: { AtomEntry } = {}
	for _, a in atoms do
		table.insert(out, clone_atom(a, a.from + char_offset, a.to + char_offset))
	end
	return out
end

return M
