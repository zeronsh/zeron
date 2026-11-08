-- Placement only: dimensions come from the composer's size contract.
local M = {}

function M.new_offset(page: number, rest: number, presented: number, foot: number): number
	local centered = 38 / 2 - 48 - rest / 2
	return if page > 0 then math.min(centered, page / 2 - foot - presented) else centered
end

function M.dock_offset(page: number, rest: number, presented: number, foot: number): number
	return page / 2 + M.new_offset(page, rest, presented, foot) + presented - (page - foot)
end

return M
