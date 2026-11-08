--!strict
-- A reverse-engineering memory view in a native window: a dump opened
-- read-only, its regions in a compact module bar, a hex view with a live
-- decode, all mock data and a Ghidra relay on the status line. The title
-- bar is the plugin's own; nothing here reads real memory, it is a faithful
-- surface to hang a real backend off.
--
--   ZERON_ZGX_DEV_PLUGINS=apps/zgx/plugins-dev/imgui-memory-inspector \
--     apps/zgx/target/debug/zeron-zgx.exe gallery

local zeron = require("./zeron")("imgui-memory-inspector")

local imgui = zeron.imgui

local window = imgui.open_window {
	id = "mem",
	title = "Memory Inspector",
	size = { width = 640, height = 408 },
	titlebar = "hidden",
}

local BAR = 30
local BUTTON = 26
local MARGIN = 10
local FOOTER = 22

type Row = { addr: number, hex: string, ascii: string, bytes: { number } }
type Region = {
	name: string,
	file: string,
	base: number,
	size: number,
	perms: string,
	rows: { Row },
}

local REGIONS: { Region } = {}

local function region(name: string, file: string, base: number, size: number, perms: string, seed: number, text: { { string } }?): Region
	local excerpt = text or {}
	local rows: { Row } = table.create(20)
	for r = 1, 20 do
		local bytes = table.create(16, 0)
		local state = seed + r * 101
		for i = 1, 16 do
			state = (state * 1103515245 + 12345) % 2147483648
			bytes[i] = math.floor(state % 256)
		end
		local line = excerpt[r]
		if line then
			local cursor = 0
			for _, word in line do
				for ch = 1, #word do
					if cursor + 1 <= 16 then
						bytes[cursor + 1] = string.byte(word, ch)
					end
					cursor += 1
				end
				cursor += 1
			end
		end
		local hex = {}
		local ascii = {}
		for i = 1, 16 do
			table.insert(hex, string.format("%02X ", bytes[i]))
			local byte = bytes[i]
			table.insert(ascii, if byte >= 32 and byte < 127 then string.char(byte) else ".")
		end
		rows[r] = {
			addr = base + (r - 1) * 16,
			hex = table.concat(hex, ""),
			ascii = table.concat(ascii, ""),
			bytes = bytes,
		}
	end
	local out: Region = { name = name, file = file, base = base, size = size, perms = perms, rows = rows }
	table.insert(REGIONS, out)
	return out
end

region(".text", "x64dbg-dump.dll", 0x00007FFA12C10000, 0x6C0, "r-x", 0x1000, {
	{ "mov", "rax,", "rcx" },
	{ "push", "rbp" },
	{ "lea", "rdx,", "[dump+0x48]" },
	{ "call", "0x7FFA12C12C0" },
})
region(".rdata", "x64dbg-dump.dll", 0x00007FFA12C30000, 0x490, "r--", 0x2000, {
	{ "x64dbg-dump.dll" },
	{ "PE32+", "executable" },
	{ "PAGE_READONLY" },
	{ "ghidra relay" },
})
region(".data", "x64dbg-dump.dll", 0x00007FFA12C40000, 0x220, "rw-", 0x3000, {
	{ "dump_session" },
	{ "section_map" },
})
region("heap [anon]", "x64dbg-dump.dll", 0x000001A8A12C0000, 0x5000, "rw-", 0x4000, {
	{ "scanned", "blocks" },
})
region("stack", "x64dbg-dump.dll", 0x0000007FFE0000, 0x2FF0, "rw-", 0x5000, {
	{ "return_addrs" },
})

local sel_region = 1
local sel_row = 1
local endian = "little"
local ascii_on = true
local frames = 0
local last_click = 0
local address = "7FFA12C10000"

local function clamp(value: number, low: number, high: number): number
	return math.max(low, math.min(high, value))
end

local function module_options(): { string }
	local out: { string } = table.create(#REGIONS)
	for at, reg in REGIONS do
		out[at] = `{reg.name}`
	end
	return out
end

-- What the first eight bytes mean, little- or big-endian, plus a float and
-- the row's text. Everything is generated, never read.
local function decode(bytes: { number }): { string }
	local little = endian ~= "big"
	local function word(count: number, at: number): number
		local value = 0
		for i = 0, count - 1 do
			local order = if little then i else count - 1 - i
			value = value + (bytes[at + i] or 0) * 256 ^ order
		end
		return value
	end
	local function float(bits: number): number
		local sign = if bits >= 2147483648 then -1 else 1
		local rest = bits % 2147483648
		local exp = math.floor(rest / 8388608)
		local mant = rest % 8388608
		if exp == 0 then
			return sign * mant / 8388608 * 2 ^ -126
		elseif exp == 255 then
			return sign * math.huge
		end
		return sign * (1 + mant / 8388608) * 2 ^ (exp - 127)
	end
	local printable = ""
	for i = 1, 16 do
		local b = bytes[i]
		printable ..= if b >= 32 and b < 127 then string.char(b) else "."
	end
	local u16 = word(2, 1)
	local u32 = word(4, 1)
	local u64 = word(8, 1)
	return {
		`byte {bytes[1]}   word {u16}   dword {u32}`,
		`qword {u64}   int {if u32 >= 2147483648 then u32 - 4294967296 else u32}`,
		`float {string.format("%.5g", float(u32))}   "{printable}"`,
	}
end

window:on_render(function(ui)
	frames += 1
	local state = window:state()
	local width = math.max(420, (tonumber(state.width) or 640) - MARGIN * 2)
	local region = REGIONS[sel_region] or REGIONS[1]
	sel_row = clamp(sel_row, 1, #region.rows)
	local row = region.rows[sel_row]

	-- Title bar: drag strip, double click to maximise, own buttons.
	local strip = ui.invisible_button { id = "titlebar", size = { width = width - BUTTON * 3 - 4, height = BAR - 6 } }
	if strip.active and ui.is_mouse_dragging() then
		window:begin_drag()
	end
	if strip.clicked then
		local now = os.clock()
		if now - last_click < 0.4 then
			last_click = 0
			if window:is_maximized() then window:restore() else window:maximize() end
		else
			last_click = now
		end
	end
	ui.same_line()
	if ui.small_button("_") then window:minimize() end
	ui.same_line()
	if ui.small_button("[]") then if window:is_maximized() then window:restore() else window:maximize() end end
	ui.same_line()
	if ui.small_button("x") then window:close() end

	local draw = ui.get_window_draw_list()

	-- Module bar: which region is open, address, and the view options.
	local chosen = ui.combo("##module", region.name, module_options())
	for at, reg in REGIONS do
		if reg.name == chosen then
			sel_region = at
			sel_row = 1
		end
	end
	ui.same_line()
	ui.text_colored("addr", "#6b6b76")
	ui.same_line()
	address = ui.input_text("##addr", address, { hint = "7FFA12C10000" })
	ui.same_line()
	if ui.small_button("Go") then
		local value = tonumber(address, 16)
		if value and value >= region.base and value < region.base + region.size then
			sel_row = clamp(1 + math.floor((value - region.base) / 16), 1, #region.rows)
		end
	end
	ui.same_line()
	endian = ui.combo("##endian", endian, { "little", "big" })
	ui.same_line()
	ascii_on = ui.checkbox("ASCII", ascii_on)

	ui.separator()

	-- Hex view: one full-width row per sixteen bytes.
	for r, line in region.rows do
		local label = `{string.format("%012X", line.addr)}  {line.hex} {if ascii_on then line.ascii else ""}`
		if ui.selectable(label, sel_row == r) then
			sel_row = r
		end
	end

	ui.separator()

	-- The decode of the selected row.
	for _, line in decode(row.bytes) do
		ui.text_colored(line, "#8fb7a0")
	end

	-- Status bar: the relay, the file, and the mock notice.
	local foot = (tonumber(state.height) or 408) - FOOTER - 4
	draw:add_rect_filled { min = { 4, foot }, max = { 4 + width + 12, foot + FOOTER }, color = "#0c0e13" }
	draw:add_circle_filled { center = { 16, foot + 11 }, radius = 4, color = "#3ddc84" }
	draw:add_text { pos = { 26, foot + 6 }, text = "Ghidra Connected - ImGUI relay", color = "#3ddc84", font_size = 12 }
	local right = `{region.file} · {endian} · {"mock data"}`
	draw:add_text { pos = { 6 + width - 7 * #right, foot + 6 }, text = right, color = "#5c5c68", font_size = 12 }
end)

zeron.tool {
	name = "memory_status",
	run = function(): any
		return {
			regions = #REGIONS,
			selected_region = sel_region,
			selected_row = sel_row,
			endian = endian,
			ascii = ascii_on,
			frames = frames,
		}
	end,
}