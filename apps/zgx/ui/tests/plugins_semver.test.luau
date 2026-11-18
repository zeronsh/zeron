-- The store's versions: strict SemVer 2.0.0 read and ordered, and the
-- ranges `engines.zeron` names read as npm reads them, a Zeron beta placed
-- by its order (screens/plugins/semver).

local semver = require("@screens/plugins/semver")

test("versions are strict SemVer: three numbers, no leading zeros, a pre-release and build read", function()
	expect(semver.parse("1.2.3")).never.to_be_nil()
	expect(semver.parse("0.4.0-beta.1")).never.to_be_nil()
	expect(semver.parse("1.0.0+build.5")).never.to_be_nil()
	for _, bad in { "1.2", "01.2.3", "1.2.3-", "1.2.3-01", "1.2.3-a..b", "1.2.3+", "x.y.z", "1.2.3.4", "" } do
		expect(`{bad}: {tostring(semver.parse(bad) ~= nil)}`).to_be(`{bad}: false`)
	end
end)

test("versions order as SemVer says, pre-releases before their release", function()
	local order = { "1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta", "1.0.0-beta.2", "1.0.0-beta.11", "1.0.0-rc.1", "1.0.0", "1.0.1", "1.1.0", "2.0.0" }
	for i = 1, #order - 1 do
		expect(`{order[i]} < {order[i + 1]}: {semver.compare(order[i], order[i + 1])}`).to_be(`{order[i]} < {order[i + 1]}: -1`)
	end
	expect(semver.compare("1.0.0+a", "1.0.0+b")).to_be(0)
end)

test("ranges take what npm's do", function()
	local cases = {
		{ "0.4.0", ">=0.4.0", true },
		{ "0.3.9", ">=0.4.0", false },
		{ "0.4.5", "^0.4.0", true },
		{ "0.5.0", "^0.4.0", false },
		{ "1.9.0", "^1.2.0", true },
		{ "2.0.0", "^1.2.0", false },
		{ "0.0.3", "^0.0.3", true },
		{ "0.0.4", "^0.0.3", false },
		{ "0.4.9", "~0.4.1", true },
		{ "0.5.0", "~0.4.1", false },
		{ "0.4.7", "0.4.x", true },
		{ "0.5.0", "0.4", false },
		{ "1.3.0", "1.2.3 - 1.4.0", true },
		{ "1.4.1", "1.2.3 - 1.4.0", false },
		{ "1.4.9", "1.2.3 - 1.4", true },
		{ "0.5.2", ">=0.4.0 <0.6.0", true },
		{ "0.6.0", ">=0.4.0 <0.6.0", false },
		{ "1.0.3", "^0.4.0 || ^1.0.0", true },
		{ "0.9.0", "^0.4.0 || ^1.0.0", false },
		{ "3.1.4", "*", true },
		{ "3.1.4", "", true },
		{ "0.4.2", ">= 0.4", true },
		{ "0.4.0", "=0.4.0", true },
		{ "0.4.0", ">0.4.0", false },
		{ "0.4.1", "<=0.4", true },
		{ "0.5.0", "<=0.4", false },
		-- A Zeron beta, by its order.
		{ "0.5.0-beta.4", ">=0.4.0", true },
		{ "0.5.0-beta.4", ">=0.5.0", false },
		{ "0.5.0-beta.4", "^0.4.0", false },
	}
	for _, c in cases do
		expect(`{c[1]} in {c[2]}: {tostring(semver.satisfies(c[1], c[2]))}`).to_be(`{c[1]} in {c[2]}: {tostring(c[3])}`)
	end
end)

test("a range or version that can't be read says so, never yes", function()
	expect(semver.satisfies("0.4.0", ">=banana")).to_be_nil()
	expect(semver.satisfies("0.4.0", "1.x.3")).to_be_nil()
	expect(semver.satisfies("dev", ">=0.4.0")).to_be_nil()
end)

test("the earliest version a range takes, to say which Zeron it needs", function()
	expect(semver.least(">=0.4.0")).to_be("0.4.0")
	expect(semver.least("^0.6.2")).to_be("0.6.2")
	expect(semver.least(">=0.5.0 <0.7.0 || >=1.2.0")).to_be("0.5.0")
	expect(semver.least("<1.0.0")).to_be_nil()
end)
