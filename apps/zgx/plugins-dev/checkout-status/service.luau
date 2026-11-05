-- Checkout status's service: run by Zeron's engine on each device that
-- allowed the plugin, with no window open. It answers `status` for the
-- plugin's windows (on this device or another of yours) and the
-- `checkout_status` tool for agents, by running this device's own `git`.

local zeron = require("./zeron")("checkout-status")

-- `git status` of the checkout at `cwd`, on this device.
local function status(cwd: string): any
	assert(type(cwd) == "string" and cwd ~= "", "status needs the checkout's folder (cwd)")
	local ran = zeron.run("git", { "-C", cwd, "status", "--porcelain=v2", "--branch" }, { timeout = 20 }):await()
	if ran.status ~= 0 then
		error(`git: {ran.stderr}`, 0)
	end
	-- `# branch.head <name>` (or "(detached)"), then a line per change.
	local branch, changes = "", 0
	for line in string.gmatch(ran.stdout, "[^\n]+") do
		local head = string.match(line, "^# branch%.head (.+)$")
		if head then
			branch = head
		elseif string.sub(line, 1, 1) ~= "#" then
			changes += 1
		end
	end
	return { device = zeron.device.name, branch = branch, changes = changes }
end

-- Reading only: safe to run again if the service stopped mid-call.
zeron.serve("status", function(args: any, call: any): any
	local answer = status(args.cwd)
	zeron.emit("checked", { cwd = args.cwd, changes = answer.changes, asked_from = call.from.name })
	return answer
end, { safe = true })

zeron.tool {
	name = "checkout_status",
	run = function(args: any): any
		return status(args.path)
	end,
}
