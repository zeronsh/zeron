-- A reply streaming in is parsed for what's new alone (ds/markdown_find
-- `parse` with a key, ds/math_source `feed`), and comes out exactly as the
-- whole text read from its start would: random markdown (math, prices,
-- fences, inline code, escapes, lists, tables, pictures, HTML, link
-- definitions) cut into random chunks, each step checked against the whole
-- read (ui/markdown_stream_fixture.luau runs the checks).

-- A check's result over `seeds` (a few at a time: a command has a time
-- budget), "ok" or what went wrong first.
local function check(name: string, seeds: number?, batch: number?): string
	local app = mount("markdown_stream_fixture.luau")
	local step = batch or 25
	for from = 1, seeds or 1, step do
		app:set_state("check.seeds", { from, from + step - 1 })
		app:command(name)
		local result = app:state("check.result")
		if result ~= "ok" then
			return result
		end
	end
	return "ok"
end

test("math read as a text grows is the math read whole, chunk by chunk", function()
	expect(check("check_math", 400)).to_be("ok")
end)

test("a reply parsed as it grows is the reply parsed whole, chunk by chunk", function()
	expect(check("check_parse", 300, 5)).to_be("ok")
end)

test("a reply's settled blocks keep their tables as it grows", function()
	expect(check("check_kept")).to_be("ok")
end)
