-- Checkout status: "Checkout status" in each session row's menu asks the
-- plugin's service on the device that session's checkout is on (this Mac,
-- or a devbox) for its branch and changes, and says so in a toast. Each
-- device runs the service only if the plugin is installed and allowed
-- there, in the same version; when it isn't, the toast says why.

local zeron = require("./zeron")("checkout-status")

local WHY: { [string]: string } = {
	offline = "isn't reachable right now",
	not_installed = "doesn't have Checkout status installed",
	not_approved = "hasn't allowed Checkout status yet",
	off = "has Checkout status turned off",
	version_mismatch = "has another version of Checkout status",
	old_engine = "needs a newer Zeron",
}

zeron.menu {
	kind = "session",
	id = "checkout-status",
	label = "Checkout status",
	run = function(row: any)
		local session = zeron.sessions()[row.id]
		if not session or not session.cwd then
			zeron.toast("This chat has no checkout")
			return
		end
		-- No device (or this one's): this Mac's service. Another: that one's.
		zeron.service(session.device):call("status", { cwd = session.cwd }):after(function(s: any)
			local changes = if s.changes == 1 then "1 change" else `{s.changes} changes`
			zeron.toast(`{s.device}: {s.branch} · {changes}`)
		end, function(err: any)
			local words = WHY[zeron.reason(err)]
			zeron.toast(if words then `That device {words}` else tostring(err))
		end)
	end,
}

-- What this device's service says it did (`zeron.emit` there).
zeron.service():on("checked", function(data: any)
	print(`checkout-status: {data.cwd} has {data.changes} changes (asked from {data.asked_from})`)
end)
