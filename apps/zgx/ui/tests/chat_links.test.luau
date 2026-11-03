-- A transcript's file links, read as Zeron reads them (crates/ui
-- workspace_links.rs, its tests' cases): screens/chat/file_links `resolve`.

local links = require("../screens/chat/file_links")
local ROOT = "/work/comet"

local function at(target: string): string?
	local r = links.resolve(target, ROOT)
	if not r then
		return nil
	end
	return `{r.path}|{r.line or "-"}|{r.column or "-"}|{if r.outside then "outside" else "in"}`
end

test("relative, absolute and file:// links resolve inside the project, with their line", function()
	expect(at("crates/ui/src/lib.rs")).to_be("crates/ui/src/lib.rs|-|-|in")
	expect(at("/work/comet/crates/ui/src/lib.rs:42:7")).to_be("crates/ui/src/lib.rs|42|7|in")
	expect(at("file:///work/comet/README.md#L12")).to_be("README.md|12|-|in")
	expect(at("zeron-file:src/a%20file%23%5Bx%5D.rs")).to_be("src/a file#[x].rs|-|-|in")
end)

test("paths decode once; unsafe decodes never resolve", function()
	expect(at("2026-09-26/Some%20Folder/it's%20here.txt")).to_be("2026-09-26/Some Folder/it's here.txt|-|-|in")
	expect(at("file:///work/comet/a%20b.md")).to_be("a b.md|-|-|in")
	expect(at("docs%2Fnote.md")).to_be("docs/note.md|-|-|in")
	expect(at("/tmp/Some%20Folder/it's%20here.txt")).to_be("/tmp/Some Folder/it's here.txt|-|-|outside")
	expect(at("a%zz.md")).to_be("a%zz.md|-|-|in")
	expect(at("a%2.md")).to_be("a%2.md|-|-|in")
	expect(at("%2E%2E/secret.md")).to_be(nil)
	expect(at("a%5Cb.md")).to_be(nil)
	expect(at("a%00b.md")).to_be(nil)
end)

test("every line form opens at its first line; zero and backwards aren't links", function()
	for target, want in
		{
			["src/lib.rs#L12"] = "12|-",
			["src/lib.rs#L12C5"] = "12|5",
			["src/lib.rs#L12-L20"] = "12|-",
			["src/lib.rs#L12-20"] = "12|-",
			["src/lib.rs#L12C1-L20C3"] = "12|1",
			["src/lib.rs:12"] = "12|-",
			["src/lib.rs:12:5"] = "12|5",
			["src/lib.rs:12-20"] = "12|-",
		}
	do
		expect(at(target)).to_be(`src/lib.rs|{want}|in`)
	end
	for _, target in
		{
			"src/lib.rs:0",
			"src/lib.rs:0:5",
			"src/lib.rs:12:0",
			"src/lib.rs:0-4",
			"src/lib.rs:4-0",
			"src/lib.rs:20-10",
			"src/lib.rs#L0",
			"src/lib.rs#L0-L3",
			"src/lib.rs#L20-L10",
			"src/lib.rs#L5-3",
			"src/lib.rs#L5-L3",
		}
	do
		expect(at(target)).to_be(nil)
	end
end)

test("a range keeps its last line, for the file to select", function()
	expect((links.resolve("src/lib.rs:10-12", ROOT) :: any).end_line).to_be(12)
	expect((links.resolve("src/lib.rs#L10-L12", ROOT) :: any).end_line).to_be(12)
	expect((links.resolve("src/lib.rs#L10C2-L14C1", ROOT) :: any).end_line).to_be(14)
	expect((links.resolve("src/lib.rs:10", ROOT) :: any).end_line).to_be(nil)
	expect((links.resolve("src/lib.rs:10-10", ROOT) :: any).end_line).to_be(nil)
end)

test("a fragment that isn't a line drops away; a # inside the path stays", function()
	expect(at("docs/readme.md#install")).to_be("docs/readme.md|-|-|in")
	expect(at("docs/readme.md#")).to_be("docs/readme.md|-|-|in")
	expect(at("docs/a.md#v1.2")).to_be("docs/a.md|-|-|in")
	expect(at("docs/we#ird/file.md")).to_be("docs/we#ird/file.md|-|-|in")
end)

test("only file-shaped paths are links", function()
	for _, target in
		{
			"~/notes.md",
			"#section",
			"?query.md",
			"src/Makefile",
			"Makefile",
			"a:b/file.md",
			"C:\\dir\\file.md",
			"C:/dir/file.md",
			"https://example.com/file.rs",
			"mailto:dev@example.com",
			"../secret.rs",
			"src/../../secret.rs",
			"src/./lib.rs",
			"src//lib.rs",
			"docs.v2/readme",
			"//work/comet/a.rs",
			"/usr/bin/ls",
			"file://localhost/work/comet/a.rs",
		}
	do
		expect(at(target)).to_be(nil)
	end
	expect(at("/work/comet-other/src/lib.rs")).to_be("/work/comet-other/src/lib.rs|-|-|outside")
	expect(at(".env")).to_be(".env|-|-|in")
	expect(at("docs/.env")).to_be("docs/.env|-|-|in")
end)

test("a reply's file links and a prompt's mention open in the session's panel", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
	app:command("link_turn")
	app:frames(30)
	app:click('role:link "the parser"')
	app:click('role:link "the host"')
	app:click('role:link ~"chat.rs"')
	-- Outside the chat's folder: styled, opening nothing.
	app:click('role:link "this one"')
	app:frames(5)
	local calls = app:state("calls")
	expect(calls).to_contain("open_file:s2:src/parse.rs")
	expect(calls).to_contain("open_file:s2:src/main.rs")
	expect(calls).to_contain("open_file:s2:src/chat.rs")
	expect(calls).never.to_contain("hosts.conf")
	-- A web address written bare (here in bold) is a link too, the full
	-- stop after it left out.
	expect(app:has('role:link "https://github.com/zeronsh/example/pull/1"')).to_be(true)
	expect(app:has('role:link "https://github.com/zeronsh/example/pull/1."')).to_be(false)
	-- A chat mentioned opens that chat.
	local _, before = string.gsub(app:state("calls"), "open:s1", "")
	app:click('role:link ~"Long chat"')
	app:frames(20)
	local _, after = string.gsub(app:state("calls"), "open:s1", "")
	expect(after).to_be(before + 1)
	expect(app:error()).to_be(nil)
end)
