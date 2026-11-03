-- A file's icon, as Zeron draws it (crates/ui/src/file_icons.rs): the
-- Symbols set, in its own colours. Its exact name first (`Cargo.toml`,
-- `.gitignore`), then its longest compound extension (`d.ts` before `ts`),
-- then the generic file; a folder by its name (`src`, `assets`), else the
-- plain folder. Dark lifts the set's darker accents, as Zeron does: each
-- icon is drawn twice, and the theme says which shows.
--
-- `FileIcon { name = "init.luau", size = 16 }`, `FileIcon { name = "src",
-- dir = true }`. `FileIcon.of(name, dir)` is the icon's path, for a list
-- that keeps its own.

local map = require("@self/file_icons_map")

-- The map's tables, looked up by any name (a name it lacks is nil). (Cast:
-- the generated map's literals type as records of their own keys alone.)
local folders = map.folders :: { [string]: string }
local names = map.names :: { [string]: string }
local extensions = map.extensions :: { [string]: string }

local cache: { [string]: string } = {}

local function of(name: string, dir: boolean?): string
	local key = if dir then "/" .. name else name
	local hit = cache[key]
	if hit then
		return hit
	end
	local lower = string.lower(name)
	local found: string?
	if dir then
		found = folders[lower] or map.folder
	else
		found = names[lower]
		if not found then
			-- `a.test.ts`: `test.ts`, then `ts`.
			local i = string.find(lower, ".", 2, true)
			while i and not found do
				found = extensions[string.sub(lower, i + 1)]
				i = string.find(lower, ".", i + 1, true)
			end
		end
	end
	local icon: string = found or map.file
	cache[key] = icon
	return icon
end

-- (Read-only: a call through the module's `__call` checks its table
-- exactly otherwise.)
export type Props = { read name: string | () -> string, read dir: (boolean | () -> boolean)?, read size: number? }

local function component(props: Props): Node
	local size = props.size or 16
	local name, dir = props.name, props.dir
	local function icon(): string
		local n = if type(name) == "function" then name() else name
		local d = if type(dir) == "function" then dir() else dir
		return of(n, d == true)
	end
	-- Both sets stacked: the theme's own shows (`zeron.files.icons`, 1 in
	-- light, 0 in dark), so a subtree themed apart from the window draws its
	-- own without the script knowing which it is in.
	local function svg(set: string, opacity: string): Node
		return ui.svg {
			src = if type(name) == "function" or type(dir) == "function"
				then function(): string
					return `assets/icons/files/{set}/{icon()}.svg`
				end
				else `assets/icons/files/{set}/{icon()}.svg`,
			w = size,
			h = size,
			absolute = true,
			left = 0,
			top = 0,
			opacity = opacity,
		}
	end
	return ui.div {
		w = size,
		h = size,
		shrink = 0,
		relative = true,
		-- (The name beside it says what it is.)
		aria_hidden = true,
		svg("light", "$zeron.files.light"),
		svg("dark", "$zeron.files.dark"),
	}
end

return setmetatable({ of = of }, {
	__call = function(_, props: Props): Node
		return component(props)
	end,
})
