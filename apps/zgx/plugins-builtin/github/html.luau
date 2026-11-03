--!strict
-- GitHub's words are markdown with HTML in them (a bot's comment is mostly
-- HTML: linked buttons, `<picture>`s for dark and light, `<sub>` lines,
-- `<details>`). Zeron's markdown drops raw HTML, so this reads the HTML
-- GitHub renders and says it as what the viewer can draw, in order:
--
--   { kind = "md", text }            markdown (links, code, emphasis, line
--                                    breaks, pictures, as markdown says them)
--   { kind = "images", items }       a paragraph of pictures alone (each
--                                    maybe a link), side by side on a line
--   { kind = "small", text }         a `<sub>` / `<sup>` paragraph: markdown,
--                                    a step smaller
--   { kind = "details", summary, body, open }   a fold: its summary (markdown),
--                                    what it holds (segments), and whether
--                                    it stands open (`<details open>`)
--
-- An image item: `src` (the `<img>`'s), `dark` / `light` (a `<picture>`'s
-- sources for each theme), `alt`, `href` (the link round it), `w` / `h` (the
-- size it asked for). Tags it doesn't know go, their words stay; comments
-- go; code (fenced or inline) is left as it is.

export type Image = { src: string, dark: string?, light: string?, alt: string, href: string?, w: number?, h: number? }
export type Segment = {
	kind: "md" | "images" | "small" | "details",
	text: string?,
	items: { Image }?,
	summary: string?,
	body: { Segment }?,
	open: boolean?,
}

local M = {}

local NAMED: { [string]: string } = { nbsp = "\u{a0}", amp = "&", lt = "<", gt = ">", quot = '"', apos = "'" }

-- Entities as the characters they stand for (in an attribute, in code).
function M.decode(s: string): string
	return (string.gsub(s, "&(#?[%w]+);", function(e: string): string?
		local hex = string.match(e, "^#[xX](%x+)$")
		if hex then
			local n = tonumber(hex, 16)
			return if n and n > 0 and n < 0x110000 then utf8.char(n) else nil
		end
		local dec = string.match(e, "^#(%d+)$")
		if dec then
			local n = tonumber(dec)
			return if n and n > 0 and n < 0x110000 then utf8.char(n) else nil
		end
		return NAMED[string.lower(e)]
	end))
end

-- An attribute of a tag (`<img src="…" …>`), decoded.
local function attr(tag: string, name: string): string?
	local t = " " .. tag
	local v = string.match(t, "%s" .. name .. "%s*=%s*\"([^\"]*)\"") or string.match(t, "%s" .. name .. "%s*=%s*'([^']*)'") or string.match(t, "%s" .. name .. "%s*=%s*([^%s\"'>]+)")
	return if v then M.decode(v) else nil
end

local function number_attr(tag: string, name: string): number?
	local v = attr(tag, name)
	return if v then tonumber(string.match(v, "^%s*(%d+)%s*$") or "") else nil
end

-- A tag's name, if `<…>` is one (`<https://…>` and `<a@b.c>` are autolinks).
local function tag_name(inner: string): (string?, boolean)
	local close, name, rest = string.match(inner, "^(/?)(%a[%w%-]*)(.*)$")
	if not name then
		return nil, false
	end
	if rest ~= "" and not string.match(rest, "^[%s/]") then
		return nil, false
	end
	return string.lower(name), close == "/"
end

local function trim(s: string): string
	return (string.match(s, "^%s*(.-)%s*$") or s)
end

-- Fenced code and inline code, set aside while the HTML round them is read
-- (and put back as they were).
local function shelter(text: string): (string, (s: string) -> string)
	local kept: { string } = {}
	local function keep(s: string): string
		table.insert(kept, s)
		return `\u{E000}{#kept}\u{E001}`
	end
	-- Fences: a line of ``` or ~~~ to the next such line.
	local out, lines, fence = {}, string.split(text, "\n"), nil :: string?
	local block: { string } = {}
	for _, line in lines do
		local mark = string.match(line, "^%s*(```+)") or string.match(line, "^%s*(~~~+)")
		if fence then
			table.insert(block, line)
			if mark and string.sub(mark, 1, 3) == string.sub(fence, 1, 3) then
				table.insert(out, keep(table.concat(block, "\n")))
				block, fence = {}, nil
			end
		elseif mark then
			fence = mark
			block = { line }
		else
			table.insert(out, line)
		end
	end
	if fence then
		table.insert(out, keep(table.concat(block, "\n")))
	end
	local body = table.concat(out, "\n")
	-- Inline code: backtick runs.
	body = string.gsub(body, "(`+)(.-)%1", function(ticks: string, inner: string): string
		return keep(ticks .. inner .. ticks)
	end)
	local function restore(s: string): string
		-- (Twice: a shelter can hold another's mark.)
		for _ = 1, 2 do
			s = string.gsub(s, "\u{E000}(%d+)\u{E001}", function(n: string): string
				return kept[tonumber(n) :: number] or ""
			end)
		end
		return s
	end
	return body, restore
end

-- `<picture>`: one `<img>`, carrying its dark and light sources.
local function pictures(text: string): string
	return (string.gsub(text, "<[pP][iI][cC][tT][uU][rR][eE][^>]*>(.-)</[pP][iI][cC][tT][uU][rR][eE]%s*>", function(inner: string): string
		local dark, light = nil :: string?, nil :: string?
		for tag in string.gmatch(inner, "<[sS][oO][uU][rR][cC][eE]([^>]*)>") do
			local media = string.lower(attr(tag, "media") or "")
			local set = attr(tag, "srcset")
			local first = if set then string.match(set, "^%s*([^%s,]+)") else nil
			if first and string.find(media, "dark", 1, true) then
				dark = first
			elseif first and string.find(media, "light", 1, true) then
				light = first
			end
		end
		local img = string.match(inner, "<[iI][mM][gG]%s[^>]*>") or ""
		local src = attr(img, "src") or light or dark
		if not src then
			return ""
		end
		local extra = ""
		if dark then
			extra ..= ` data-dark="{dark}"`
		end
		if light then
			extra ..= ` data-light="{light}"`
		end
		local rest = string.gsub(string.gsub(img, "^<[iI][mM][gG]", ""), "/?>$", "")
		rest = string.gsub(rest, "%ssrc%s*=%s*\"[^\"]*\"", "")
		rest = string.gsub(rest, "%ssrc%s*=%s*'[^']*'", "")
		return `<img src="{src}"{extra}{rest}>`
	end))
end

local function image_of(tag: string, href: string?): Image?
	local src = attr(tag, "src")
	if not src or src == "" then
		return nil
	end
	return {
		src = src,
		dark = attr(tag, "data-dark"),
		light = attr(tag, "data-light"),
		alt = attr(tag, "alt") or "",
		href = href,
		w = number_attr(tag, "width"),
		h = number_attr(tag, "height"),
	}
end

-- A paragraph that is pictures alone (each maybe in a link), apart by
-- spaces, `&nbsp;` or line breaks: its pictures, in order; else nil.
local function only_images(chunk: string): { Image }?
	local items: { Image } = {}
	local href: string? = nil
	local at = 1
	local s = chunk
	while at <= #s do
		local ws = string.match(s, "^[%s]+", at) or string.match(s, "^&nbsp;", at) or string.match(s, "^\u{a0}", at)
		if ws then
			at += #ws
		else
			local whole, inner = string.match(s, "^(<([^>]*)>)", at)
			local md_linked, md_alt, md_src, md_href = nil, nil, nil, nil
			if not whole then
				md_linked, md_alt, md_src, md_href = string.match(s, "^(%[!%[([^%]]*)%]%(%s*([^%s%)]+)[^%)]*%)%]%(%s*([^%s%)]+)%s*%))", at)
			end
			if whole then
				local name, close = tag_name(inner)
				if name == "a" and not close then
					href = attr(inner, "href")
				elseif name == "a" and close then
					href = nil
				elseif name == "img" then
					local img = image_of(inner, href)
					if img then
						table.insert(items, img)
					end
				elseif name ~= "br" and name ~= "p" and name ~= "div" then
					return nil
				end
				at += #whole
			elseif md_linked then
				table.insert(items, { src = md_src, alt = md_alt, href = md_href })
				at += #md_linked
			else
				local plain, alt, src = string.match(s, "^(!%[([^%]]*)%]%(%s*([^%s%)]+)[^%)]*%))", at)
				if not plain then
					return nil
				end
				table.insert(items, { src = src, alt = alt, href = href })
				at += #plain
			end
		end
	end
	return if #items > 0 then items else nil
end

-- What a markdown picture says for an image (its size kept by the caller).
local function md_image(img: Image): string
	local alt = string.gsub(img.alt, "[%[%]]", "")
	return `![{alt}]({img.src})`
end

-- HTML said as markdown, inline: links, pictures, code, emphasis, breaks,
-- headings, list items; other tags go, their words stay.
function M.inline(text: string, on_image: ((img: Image) -> ())?): string
	local s = text
	-- Links round anything.
	s = string.gsub(s, "<[aA](%s[^>]*)>(.-)</[aA]%s*>", function(tag: string, inner: string): string
		local href = attr(tag, "href")
		local body = trim(M.inline(inner, on_image))
		if not href or href == "" then
			return body
		end
		return `[{body}]({href})`
	end)
	s = string.gsub(s, "<[iI][mM][gG](%s[^>]*)>", function(tag: string): string
		local img = image_of(tag, nil)
		if not img then
			return ""
		end
		if on_image then
			on_image(img)
		end
		return md_image(img)
	end)
	s = string.gsub(s, "<[cC][oO][dD][eE][^>]*>(.-)</[cC][oO][dD][eE]%s*>", function(inner: string): string
		local code = M.decode(string.gsub(inner, "<[^>]*>", ""))
		local ticks = if string.find(code, "`", 1, true) then "``" else "`"
		return `{ticks}{code}{ticks}`
	end)
	s = string.gsub(s, "<([^>]*)>", function(inner: string): string?
		local name, close = tag_name(inner)
		if not name then
			return nil
		end
		if name == "b" or name == "strong" then
			return "**"
		elseif name == "i" or name == "em" then
			return "*"
		elseif name == "s" or name == "del" or name == "strike" then
			return "~~"
		elseif name == "br" then
			-- (Two spaces and a newline: a hard break with words after it,
			-- nothing at all alone at a line's end, where a backslash would
			-- show.)
			return "  \n"
		elseif name == "hr" then
			return "\n\n---\n\n"
		elseif name == "li" then
			return if close then "\n" else "\n- "
		elseif string.match(name, "^h[1-6]$") then
			return if close then "\n\n" else "\n\n" .. string.rep("#", tonumber(string.sub(name, 2)) :: number) .. " "
		elseif name == "p" or name == "div" or name == "ul" or name == "ol" or name == "table" or name == "blockquote" then
			return "\n\n"
		end
		return ""
	end)
	return s
end

-- The segments of what GitHub says, in order.
function M.segments(text: string, on_image: ((img: Image) -> ())?): { Segment }
	local body, restore = shelter((string.gsub(text or "", "\r\n", "\n")))
	body = string.gsub(body, "<!%-%-.-%-%->", "")
	body = pictures(body)
	local out: { Segment } = {}
	local md: { string } = {}
	local function flush()
		if #md > 0 then
			local joined = trim(table.concat(md, "\n\n"))
			if joined ~= "" then
				table.insert(out, { kind = "md", text = restore(joined) })
			end
			md = {}
		end
	end
	local function paragraphs(part: string)
		-- Paragraphs: blank lines, and the HTML that makes them.
		part = string.gsub(part, "<(/?)([pP])(%s?[^>]*)>", function(_c: string, _n: string, rest: string): string?
			return if rest == "" or string.match(rest, "^%s") then "\n\n" else nil
		end)
		part = string.gsub(part, "</?[dD][iI][vV][^>]*>", "\n\n")
		-- A run of text as a line of pictures, a small line, or neither (nil).
		local function special(c: string): Segment?
			local images = only_images(c)
			local small = string.match(c, "^<[sS][uU][bBpP]>(.*)</[sS][uU][bBpP]>$")
			local single_plain = images and #images == 1 and not images[1].href and not images[1].dark and not images[1].light
			if images and not single_plain then
				return { kind = "images", items = images }
			elseif images then
				-- A picture alone on its line (a screenshot under a word):
				-- a picture of its own, as GitHub draws it.
				if on_image then
					on_image(images[1])
				end
				return { kind = "md", text = md_image(images[1]) }
			elseif small and not string.find(small, "</?[sS][uU][bBpP]>") then
				return { kind = "small", text = restore(trim(M.inline(small, on_image))) }
			end
			return nil
		end
		for _, chunk in string.split(part, "\n\n") do
			local c = trim(chunk)
			if c ~= "" then
				local whole = special(c)
				if whole and whole.kind == "md" then
					table.insert(md, whole.text :: string)
				elseif whole then
					flush()
					table.insert(out, whole)
				else
					-- A paragraph mixing them (a bot's buttons, then its small
					-- print on the next line): each line as what it is, the
					-- markdown lines between kept together.
					local lines = string.split(c, "\n")
					local found: { Segment? } = {}
					local any = false
					for i, line in lines do
						found[i] = if #lines > 1 then special(trim(line)) else nil
						any = any or found[i] ~= nil
					end
					if not any then
						table.insert(md, (M.inline(c, on_image)))
					else
						local run: { string } = {}
						local function run_out()
							if #run > 0 then
								table.insert(md, (M.inline(table.concat(run, "\n"), on_image)))
								run = {}
							end
						end
						for i, line in lines do
							local seg = found[i]
							if seg and seg.kind == "md" then
								run_out()
								table.insert(md, seg.text :: string)
							elseif seg then
								run_out()
								flush()
								table.insert(out, seg)
							else
								table.insert(run, line)
							end
						end
						run_out()
					end
				end
			end
		end
	end
	-- `<details>`: a fold, nesting allowed.
	local at = 1
	while true do
		local s, e = string.find(body, "<[dD][eE][tT][aA][iI][lL][sS][^>]*>", at)
		if not s then
			paragraphs(string.sub(body, at))
			break
		end
		-- (`<details open>`: shown open; else folded, as GitHub folds it.)
		local tag = string.sub(body, s, e)
		local stands_open = string.match(tag, "^<[dD][eE][tT][aA][iI][lL][sS]%s[^>]*%f[%w]open%f[^%w%-]") ~= nil
		paragraphs(string.sub(body, at, s - 1))
		local depth, scan, close_s, close_e = 1, (e :: number) + 1, nil :: number?, nil :: number?
		while depth > 0 do
			local os_, oe = string.find(body, "<[dD][eE][tT][aA][iI][lL][sS][^>]*>", scan)
			local cs, ce = string.find(body, "</[dD][eE][tT][aA][iI][lL][sS]%s*>", scan)
			if not cs then
				break
			end
			if os_ and os_ < cs then
				depth += 1
				scan = (oe :: number) + 1
			else
				depth -= 1
				scan = (ce :: number) + 1
				if depth == 0 then
					close_s, close_e = cs, ce
				end
			end
		end
		local inner = string.sub(body, (e :: number) + 1, (close_s or (#body + 1)) - 1)
		local summary = string.match(inner, "<[sS][uU][mM][mM][aA][rR][yY][^>]*>(.-)</[sS][uU][mM][mM][aA][rR][yY]%s*>")
		local rest = string.gsub(inner, "<[sS][uU][mM][mM][aA][rR][yY][^>]*>.-</[sS][uU][mM][mM][aA][rR][yY]%s*>", "", 1)
		flush()
		table.insert(out, {
			kind = "details",
			-- (No summary: "", for the viewer to say "Details" in its words.)
			summary = restore(trim(M.inline(summary or "", on_image))),
			body = M.segments(restore(rest), on_image),
			open = stands_open,
		})
		at = if close_e then close_e + 1 else #body + 1
	end
	flush()
	return out
end

return M
