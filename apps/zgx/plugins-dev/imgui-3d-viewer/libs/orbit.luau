--!strict
-- The viewer's orbit camera, as a library: `imgui.library("orbit")`.
--
-- A library in a plugin's `libs/` folder is handed the plugin's own
-- `zeron.imgui` entry point when it is first asked for, so the widget code
-- here draws with exactly the API the plugin's `init.luau` uses. It is
-- loaded once: the table below is what `imgui.library` hands back every
-- time after that.
--
-- A camera you turn by dragging: yaw and pitch from the pointer's movement,
-- a distance the caller may set, and a reset.

return function(imgui: any): any
	local camera = {
		yaw = 0.7,
		pitch = -0.42,
		distance = 9.2,
		spinning = true,
	}
	local last_click = 0

	-- Turn the camera by the pointer's movement this frame, and stop the
	-- spin: the drag is the person's, not the clock's.
	function camera.drag(ui: any, view: any): boolean
		if not (view.active and ui.is_mouse_dragging()) then
			return false
		end
		local dx, dy = ui.mouse_delta()
		if dx == 0 and dy == 0 then
			return false
		end
		camera.yaw -= dx * 0.01
		camera.pitch = math.max(-1.4, math.min(1.4, camera.pitch + dy * 0.01))
		camera.spinning = false
		return true
	end

	-- A click on the scene starts the spin again.
	function camera.clicked(view: any)
		if not view.clicked then
			return
		end
		local now = os.clock()
		if now - last_click < 0.4 then
			camera.reset()
			last_click = 0
			return
		end
		last_click = now
		camera.spinning = not camera.spinning
	end

	function camera.turn(dt: number)
		if camera.spinning then
			camera.yaw += dt * 0.4
			if camera.yaw > math.pi * 2 then
				camera.yaw -= math.pi * 2
			end
		end
	end

	function camera.reset()
		camera.yaw = 0.7
		camera.pitch = -0.42
		camera.distance = 9.2
		camera.spinning = true
	end

	function camera.place(): any
		return { yaw = camera.yaw, pitch = camera.pitch, distance = camera.distance, fov = 42 }
	end

	return camera
end