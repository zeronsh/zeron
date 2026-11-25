-- How many working crew threads the side panel's Crew tab shows
-- (ds/orch_crew_fit): at least ten, then a row at a time as room allows,
-- never past how many there are nor its ceiling.

local Fit = require("@ds/orch_crew_fit")

test("crew fit: as it is till a row's height is known", function()
	expect(Fit.next(10, 30, 500, 0)).to_be(10)
end)

test("crew fit: grows by the rows the room holds, never past how many there are", function()
	expect(Fit.next(10, 30, 200, 60)).to_be(13)
	expect(Fit.next(10, 12, 2000, 60)).to_be(12)
	-- Less room than a row: as it is.
	expect(Fit.next(13, 30, 59, 60)).to_be(13)
end)

test("crew fit: shrinks by the rows that overflow, never below ten", function()
	expect(Fit.next(15, 30, -70, 60)).to_be(13)
	expect(Fit.next(11, 30, -500, 60)).to_be(10)
	expect(Fit.next(10, 30, -500, 60)).to_be(Fit.MIN)
end)

test("crew fit: never past its ceiling", function()
	expect(Fit.next(10, 100, 100000, 60)).to_be(Fit.MAX)
	expect(Fit.MIN >= 10).to_be(true)
	expect(Fit.MAX > Fit.MIN).to_be(true)
end)
