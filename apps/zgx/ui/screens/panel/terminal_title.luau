-- A Windows console may prefix its executable's full path with an
-- elevation label. Keep that label and any running command, shortening
-- only the executable path. Other shell titles stay as they are.
return function(title: string): string
	local prefix, program, running = string.match(title, "^(.-)%a:\\.-([^\\]-)%.[Ee][Xx][Ee](.*)$")
	return if program and program ~= "" then prefix .. program .. running else title
end
