local short_title = require("../screens/panel/terminal_title")

test("terminal title: Windows program paths keep their elevation prefix and running command", function()
	local cases = {
		{ [[C:\WINDOWS\system32\cmd.exe]], "cmd" },
		{ [[Administrator: C:\WINDOWS\system32\cmd.exe]], "Administrator: cmd" },
		{ [[C:\Program Files\PowerShell\7\pwsh.EXE - npm run dev]], "pwsh - npm run dev" },
		{ "bash - npm run dev", "bash - npm run dev" },
	}
	for _, case in cases do
		expect(short_title(case[1])).to_be(case[2])
	end
end)
