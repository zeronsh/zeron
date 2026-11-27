local control = require("@self/control")
local motion = require("@self/motion")

local HEIGHT, RADIUS, RANGE = 216, 96, 8
local CENTER, TICK_HEIGHT, HIT_SLOP, DRAG_SLOP = HEIGHT / 2, 2, 12, 5
local ROLL_MS = 210
local WHEEL_IDLE, WHEEL_ROLL_MS = 220, 180
local DRAG_STEP = RADIUS / RANGE * (0.72 * math.pi / 2 + 0.28)

local function project(distance: number): number
	local depth = distance / RANGE
	return CENTER + RADIUS * (0.72 * math.sin(depth * math.pi / 2) + 0.28 * depth)
end

local function bounds(at: number, count: number): (number, number)
	return math.max(1, math.ceil(at - RANGE)), math.min(count, math.floor(at + RANGE))
end

local function nearest(y: number, at: number, count: number): number?
	local first, last = bounds(at, count)
	local best: number? = nil
	local gap = HIT_SLOP
	for i = first, last do
		if math.abs(i - at) < RANGE then
			local dy = math.abs(y - project(i - at))
			if dy <= gap then
				best, gap = i, dy
			end
		end
	end
	return best
end

local function excerpt(text: string): string
	text = string.gsub(text, "%s+", " "):match("^%s*(.-)%s*$") or ""
	if text == "" then
		return "Attachment-only prompt"
	end
	local _, first = text:find("[%.%?!][\"']?%s")
	local second: number? = nil
	if first then
		local _, finish = text:find("[%.%?!][\"']?%s", first + 1)
		second = finish
	end
	local limit = utf8.offset(text, 181)
	local last = math.min(if second then second - 1 else #text, if limit then limit - 1 else #text)
	return string.sub(text, 1, last) .. (if last < #text then "…" else "")
end

type Target = { index: number, id: string }
type Press = { x: number, y: number, target: Target?, moved: boolean }

return ui.component(function(props: any): Node
	local position = ui.use_state(props.position:peek())
	local hover_y = ui.use_state(nil :: number?)
	local pressed = ui.use_state(nil :: number?)
	local centered = ui.use_state(false)
	local press: Press? = nil
	local stop_pending: (() -> ())? = nil
	local finish_roll: (() -> ())? = nil
	local wheel_idle: (() -> ())? = nil
	local wheel_points = 0
	local wheel_target: number? = nil
	local wheel_seek: ((boolean) -> ())? = nil
	local platform = ui.shared("platform", "windows"):peek()
	local wheel_step = if platform == "windows" then 100 elseif platform == "macos" then 30 else 40
	local last_y, last_time, velocity = 0, 0, 0
	local prompts = props.prompts
	local function inside(e: PointerEvent): boolean
		return e.local_x >= 0 and e.local_x <= props.gutter:peek() and e.local_y >= 0 and e.local_y <= HEIGHT
	end

	local function current_session(): string?
		return if props.session then props.session:peek() else nil
	end
	local session = current_session()
	local function reset_wheel()
		if wheel_idle then
			wheel_idle()
			wheel_idle = nil
		end
		wheel_points = 0
		wheel_target = nil
		wheel_seek = nil
	end
	local function stop()
		reset_wheel()
		local cancel = stop_pending
		stop_pending, finish_roll = nil, nil
		if cancel then
			cancel()
		end
	end
	local function clear_press()
		press = nil
		pressed:set(nil)
	end
	local function move(value: number)
		position:set(math.clamp(value, 1, math.max(1, prompts:len())))
	end
	local function scrub(value: number)
		local all = prompts:peek()
		if current_session() ~= session or #all < 2 then
			return
		end
		local previous = math.round(position:peek())
		local next_position = math.clamp(value, 1, #all)
		local index = math.round(next_position)
		position:set(next_position)
		if previous ~= index then
			props.scrub(all[index].id, all[index].row)
		end
	end
	local function finish_wheel()
		if wheel_idle or wheel_seek then
			return
		end
		reset_wheel()
		centered:set(false)
	end
	local function roll_wheel(steps: number)
		local all = prompts:peek()
		if current_session() ~= session or #all < 2 then
			return
		end
		local from = wheel_target or math.round(position:peek())
		local target = math.clamp(from + steps, 1, #all)
		if target == from then
			return
		end
		local start = position:peek()
		wheel_target = target
		centered:set(true)
		local function seek_done(landed: boolean)
			if wheel_seek ~= seek_done then
				return
			end
			wheel_seek = nil
			if landed then
				move(target)
				finish_wheel()
			else
				reset_wheel()
				centered:set(false)
			end
		end
		wheel_seek = seek_done
		local prompt = all[target]
		props.scrub(prompt.id, prompt.row, WHEEL_ROLL_MS, seek_done, function(progress: number)
			if wheel_seek == seek_done then
				move(start + (target - start) * progress)
			end
		end)
	end
	local function abandon()
		stop()
		props.cancel_scrub()
		clear_press()
		centered:set(false)
		hover_y:set(nil)
		move(props.position:peek())
	end
	local function jump(value: number, keep_hover: boolean?)
		stop()
		clear_press()
		if not keep_hover then
			hover_y:set(nil)
		end
		move(math.round(value))
		local prompt = prompts:peek()[position:peek()]
		if prompt and current_session() == session then
			props.jump(prompt.id)
		end
		centered:set(false)
	end
	local function index_of(id: string, near: number): number?
		local all = prompts:peek()
		if all[near] and all[near].id == id then
			return near
		end
		for i, prompt in all do
			if prompt.id == id then
				return i
			end
		end
		return nil
	end
	local function roll(target: number, id: string)
		stop()
		if motion.reduced_now() or target == position:peek() then
			jump(target, true)
			return
		end
		local prompt = prompts:peek()[target]
		if not prompt or prompt.id ~= id or current_session() ~= session then
			abandon()
			return
		end
		local function finish_instantly()
			local index = index_of(id, target)
			if index and current_session() == session then
				jump(index)
			else
				abandon()
			end
		end
		local start = position:peek()
		centered:set(true)
		finish_roll = finish_instantly
		props.scrub(id, prompt.row, ROLL_MS, function(landed: boolean)
			if finish_roll ~= finish_instantly then
				return
			end
			if landed then
				move(target)
				finish_roll = nil
				centered:set(false)
			else
				abandon()
			end
		end, function(progress: number)
			if finish_roll == finish_instantly then
				move(start + (target - start) * progress)
			end
		end)
	end
	local function settle_scrub()
		move(math.round(position:peek()))
		centered:set(false)
	end
	local function coast()
		stop_pending = nil
		if current_session() ~= session or #prompts:peek() < 2 then
			abandon()
			return
		end
		local before = position:peek()
		if motion.reduced_now() or math.abs(velocity) < 0.014 then
			settle_scrub()
			return
		end
		scrub(before + velocity * 16)
		velocity *= 0.8
		if before == position:peek() then
			settle_scrub()
		else
			stop_pending = ui.after(16, coast)
		end
	end

	local previous_count = 0
	ui.effect(function()
		local next_session = if props.session then props.session:get() else nil
		local followed = props.position:get()
		local count = prompts:len()
		local count_changed = count ~= previous_count
		previous_count = count
		local wheeling = wheel_idle ~= nil or wheel_seek ~= nil
		local scrubbing = centered:peek() and not finish_roll
		if next_session ~= session or (count_changed and (count < 2 or press or wheeling or scrubbing)) then
			session = next_session
			abandon()
		elseif not press and not wheeling and not centered:peek() then
			move(followed)
		end
	end)
	ui.effect(function()
		if not motion.reduced() then
			return
		end
		if finish_roll then
			finish_roll()
		end
	end)
	ui.on_cleanup(stop)

	local selected = ui.computed(function(): number?
		local at, count = position:get(), prompts:len()
		if count < 2 then
			return nil
		end
		if centered:get() then
			return math.clamp(math.round(at), 1, count)
		end
		local y = hover_y:get()
		return if y then nearest(y, at, count) else nil
	end)
	local preview = ui.use_state("")
	local preview_y = ui.use_state(CENTER - 20)
	local previous_text: string? = nil
	ui.effect(function()
		local index = selected:get()
		local prompt = if index then prompts:at(index) else nil
		if not index or not prompt then
			return
		end
		local anchor = if centered:get() then CENTER else project(index - position:get())
		preview_y:set(math.clamp(anchor - 20, 0, HEIGHT - 72))
		local text = prompt.text or ""
		if previous_text ~= text then
			previous_text = text
			preview:set(excerpt(text))
		end
	end)
	local preview_width = ui.computed(function(): number
		return math.clamp(props.view_width() - props.gutter:get() - 16, 0, 248)
	end)
	local ticks = ui.computed(function()
		local first, last = bounds(position:get(), prompts:len())
		local items = {}
		for i = first, last do
			table.insert(items, i)
		end
		return items
	end)

	return ui.presence(function()
		local count = prompts:len()
		return if count > 1 then tostring(count) else nil
	end, function(count: string): Node
		local preview_opacity = ui.tween(ui.computed(function(): number
			return if selected:get() and preview_width:get() >= 112 then 1 else 0
		end), { duration = 100, ease = "out(3)" })
		return ui.div {
			id = "prompt-jump-rail",
			relative = true,
			w_full = true,
			h_full = true,
			focusable = true,
			focus = control.ring,
			role = "slider",
			min = 1,
			max = tonumber(count),
			step = 1,
			value = function(): number return math.clamp(math.round(position:get()), 1, math.max(1, prompts:len())) end,
			on_change = function(value: number) jump(value) end,
			label = "Jump to prompt",
			description = "Use the arrow keys to jump between prompts, or Home and End for the first and last prompt.",
			value_text = function(): string
				local total = prompts:len()
				local index = math.clamp(math.round(position:get()), 1, math.max(1, total))
				local prompt = prompts:at(index)
				local line = prompt and string.match(prompt.text or "", "^[^\r\n]+")
				local value = `Prompt {index} of {total}`
				return if line then `{value}: {line}` else value
			end,
			cursor = "grab",
			on_blur = function()
				if press or stop_pending or finish_roll or wheel_idle or wheel_seek then
					abandon()
				end
			end,
			on_key = function(e: KeyEvent): boolean
				if e.cmd or e.alt or e.ctrl or e.meta then
					return false
				end
				local current = math.round(position:peek())
				if e.key == "ArrowUp" or e.key == "ArrowLeft" then
					jump(current - 1)
				elseif e.key == "ArrowDown" or e.key == "ArrowRight" then
					jump(current + 1)
				elseif e.key == "Home" then
					jump(1)
				elseif e.key == "End" then
					jump(#prompts:peek())
				elseif e.key == "Escape" and (press or stop_pending or finish_roll or wheel_idle or wheel_seek) then
					abandon()
				else
					return false
				end
				return true
			end,
			on_wheel = function(e: WheelEvent): boolean
				if e.cmd or math.abs(e.dy) <= math.abs(e.dx) then
					return false
				end
				if wheel_idle then
					wheel_idle()
				elseif not wheel_seek then
					if finish_roll then
						abandon()
					else
						stop()
					end
					clear_press()
					hover_y:set(nil)
				end
				wheel_idle = ui.after(WHEEL_IDLE, function()
					wheel_idle = nil
					wheel_points = 0
					finish_wheel()
				end)
				wheel_points += e.dy
				local steps = math.round(wheel_points / wheel_step)
				if steps ~= 0 then
					wheel_points -= steps * wheel_step
					roll_wheel(steps)
				end
				return true
			end,
			on_press = function(e: PointerEvent)
				stop()
				props.cancel_scrub()
				centered:set(false)
				hover_y:set(e.local_y)
				local all = prompts:peek()
				local index = nearest(e.local_y, position:peek(), #all)
				local target = if index then { index = index, id = all[index].id } else nil
				press = { x = e.x, y = e.y, target = target, moved = false }
				pressed:set(index)
				last_y, last_time, velocity = e.y, ui.now(), 0
			end,
			on_hover = function(inside: boolean)
				if not inside then
					hover_y:set(nil)
				end
			end,
			on_pointer_move = function(e: PointerEvent)
				local held = press
				if not held then
					if not centered:peek() then
						hover_y:set(if inside(e) then e.local_y else nil)
					end
					return
				end
				if math.abs(e.x - held.x) >= DRAG_SLOP then
					held.target = nil
					pressed:set(nil)
				end
				if not held.moved and math.abs(e.y - held.y) < DRAG_SLOP then
					return
				end
				held.moved = true
				pressed:set(nil)
				hover_y:set(nil)
				centered:set(true)
				if e.y ~= last_y then
					local now = ui.now()
					local step = (last_y - e.y) / DRAG_STEP
					scrub(position:peek() + step)
					velocity = math.clamp(step / math.max(8, now - last_time), -0.085, 0.085)
					last_y, last_time = e.y, now
				end
			end,
			on_release = function(e: PointerEvent)
				local held = press
				if not held then
					return
				end
				clear_press()
				if current_session() ~= session then
					abandon()
				elseif held.moved then
					if ui.now() - last_time > 80 then
						velocity = 0
					end
					coast()
				elseif held.target and math.abs(e.x - held.x) < DRAG_SLOP and math.abs(e.y - held.y) < DRAG_SLOP and inside(e) then
					local target = held.target
					local index = index_of(target.id, target.index)
					if index then
						roll(index, target.id)
					else
						abandon()
					end
				else
					abandon()
				end
			end,
			ui.each(ticks, function(index: number): Node
				local function distance(): number
					return index - position:get()
				end
				local function center_weight(): number
					return math.max(0, 1 - math.abs(distance()))
				end
				return ui.div {
					absolute = true,
					left = "20%",
					top = 0,
					y = function(): number return project(distance()) - TICK_HEIGHT / 2 end,
					w = "30%",
					h = TICK_HEIGHT,
					origin = { 0, 0.5 },
					scale_x = function(): number
						return (1 + center_weight()) * (if pressed:get() == index then 0.85 else 1)
					end,
					opacity = function(): number
						local edge = RANGE - math.abs(distance())
						local ink = 0.32 + 0.16 * edge / RANGE + 0.52 * center_weight()
						local hovered = not centered:get() and selected:get() == index
						return math.max(ink, if hovered then 0.78 else 0) * math.clamp(edge * 1.5, 0, 1)
					end,
					rounded = "$radius.full",
					bg = "$text.primary",
					aria_hidden = true,
					pointer_events = "none",
				}
			end, { key = function(index: number): number return index end }),
			ui.div {
				id = "prompt-jump-preview",
				absolute = true,
				left = function(): number return props.gutter:get() + 6 end,
				top = 0,
				y = preview_y,
				w = preview_width,
				p = "$space.2",
				bg = "$background",
				border = 1,
				border_color = "$border.subtle",
				rounded = "$radius.md",
				pointer_events = "none",
				aria_hidden = true,
				selectable = false,
				overflow = "hidden",
				opacity = preview_opacity,
				ui.text {
					function(): string return preview:get() end,
					literal = true,
					wrap = true,
					text_size = "$font.size.sm",
					color = "$text.primary",
					pointer_events = "none",
					selectable = false,
				},
			},
		}
	end, { absolute = true, left = 0, top = "50%", y = -CENTER, w = props.gutter, h = HEIGHT, z = 1 })
end)
