--!strict

export type Size = {
	width: number,
	height: number,
}

export type Point = {
	x: number,
	y: number,
}

export type Colour = string

export type Rgb = {
	r: number,
	g: number,
	b: number,
}

export type TextureHandle = any

export type MeshHandle = any

export type SceneOptions = {
	id: string,
	width: number,
	height: number,
	camera: { yaw: number?, pitch: number?, distance: number?, fov: number?, target: { number }? }?,
	models: { { mesh: MeshHandle, position: { number }?, rotation: { number }?, scale: number | { number }?, color: Colour? } },
}

export type SceneAnswer = {
	hovered: boolean,
	active: boolean,
	clicked: boolean,
	min: Point,
	max: Point,
}

export type FontHandle = any

export type DrawList = {
	add_line: (self: DrawList, options: { from: { number }, to: { number }, color: Colour, thickness: number? }) -> (),
	add_rect: (self: DrawList, options: { min: { number }, max: { number }, color: Colour, thickness: number?, rounding: number? }) -> (),
	add_rect_filled: (self: DrawList, options: { min: { number }, max: { number }, color: Colour, rounding: number? }) -> (),
	add_rect_rounded: (self: DrawList, options: { min: { number }, max: { number }, color: Colour, rounding: number, thickness: number? }) -> (),
	add_rect_rounded_filled: (self: DrawList, options: { min: { number }, max: { number }, color: Colour, rounding: number }) -> (),
	add_circle: (self: DrawList, options: { center: { number }, radius: number, color: Colour, thickness: number? }) -> (),
	add_circle_filled: (self: DrawList, options: { center: { number }, radius: number, color: Colour }) -> (),
	add_text: (self: DrawList, options: { pos: { number }, text: string, color: Colour, font_size: number? }) -> (),
	add_image: (self: DrawList, options: { handle: TextureHandle, min: { number }, max: { number }, uv_min: { number }?, uv_max: { number }?, color: Colour? }) -> (),
	add_poly_filled: (self: DrawList, options: { points: { { number } }, color: Colour }) -> (),
	add_poly_line: (self: DrawList, options: { points: { { number } }, color: Colour, thickness: number?, closed: boolean? }) -> (),
	push_clip_rect: (self: DrawList, options: { min: { number }, max: { number } }) -> (),
	pop_clip_rect: (self: DrawList) -> (),
	clear: (self: DrawList) -> (),
}

export type ImageOptions = {
	handle: TextureHandle,
	width: number,
	height: number,
}

export type ImageAnswer = {
	ok: boolean,
	min: Point,
	max: Point,
}

export type ImGuiStyle = {
	theme: string,
	colours: { [string]: string },
	frame_rounding: number,
	window_rounding: number,
	window_padding: Point,
	item_spacing: Point,
	border_size: number,
	text_size: number,
	dpi_scale: number,
	font: string?,
}

export type StyleOptions = {
	theme: "auto" | "dark" | "light"?,
	colours: { [string]: string }?,
	frame_rounding: number?,
	window_rounding: number?,
	window_padding: Point?,
	item_spacing: Point?,
	border_size: number?,
	text_size: number?,
	font: FontHandle?,
}

export type WindowState = {
	open: boolean,
	shown: boolean,
	minimized: boolean,
	frames: number,
	id: string?,
	title: string?,
	maximized: boolean?,
	width: number?,
	height: number?,
}

export type WindowOptions = {
	id: string,
	title: string,
	size: Size?,
	position: Point?,
	min_size: Size?,
	max_size: Size?,
	resizable: boolean?,
	visible: boolean?,
	titlebar: "standard" | "inset" | "hidden"?,
	decorations: boolean?,
	remember: boolean?,
}

export type InputTextOptions = {
	hint: string?,
	password: boolean?,
}

export type MultilineOptions = {
	width: number,
	height: number,
}

export type RangeOptions = {
	min: number,
	max: number,
}

export type DragOptions = {
	speed: number?,
	min: number?,
	max: number?,
}

export type SelectableOptions = {
	span_all_columns: boolean?,
}

export type InvisibleButtonOptions = {
	id: string,
	size: Size,
}

export type InvisibleButtonAnswer = {
	hovered: boolean,
	active: boolean,
	clicked: boolean,
	min: Point,
	max: Point,
}

export type ChildOptions = {
	width: number,
	height: number,
	border: boolean?,
}

export type SortSpec = {
	column: string,
	sort: string,
}

export type Ui = {
	text: (value: string) -> (),
	button: (label: string) -> boolean,
	text_colored: (value: string, colour: Colour) -> (),
	separator: () -> (),
	small_button: (label: string) -> boolean,
	checkbox: (label: string, on: boolean?) -> boolean,
	radio_button: (label: string, active: boolean?) -> boolean,
	input_text: (label: string, value: string, options: InputTextOptions?) -> string,
	input_text_multiline: (label: string, value: string, options: MultilineOptions) -> string,
	input_int: (label: string, value: number, options: RangeOptions) -> number,
	input_float: (label: string, value: number, options: RangeOptions) -> number,
	slider_int: (label: string, value: number, options: RangeOptions) -> number,
	slider_float: (label: string, value: number, options: RangeOptions) -> number,
	drag_int: (label: string, value: number, options: DragOptions?) -> number,
	drag_float: (label: string, value: number, options: DragOptions?) -> number,
	progress_bar: (fraction: number) -> (),
	combo: (label: string, value: string, options: { string }) -> string,
	selectable: (label: string, selected: boolean?, options: SelectableOptions?) -> boolean,
	bullet_text: (value: string) -> (),
	invisible_button: (options: InvisibleButtonOptions) -> InvisibleButtonAnswer,
	is_item_hovered: () -> boolean,
	is_item_active: () -> boolean,
	mouse_delta: () -> (number, number),
	is_mouse_dragging: (button: string?) -> boolean,
	same_line: () -> (),
	new_line: () -> (),
	spacing: () -> (),
	dummy: (size: Size) -> (),
	indent: () -> (),
	unindent: () -> (),
	begin_child: (id: string, size: ChildOptions?) -> (),
	end_child: () -> (),
	columns: (count: number) -> (),
	next_column: () -> (),
	begin_group: () -> (),
	end_group: () -> (),
	set_next_item_width: (width: number) -> (),
	begin_table: (options: { [string]: any }) -> boolean,
	table_setup_column: (options: { [string]: any }) -> (),
	table_headers_row: () -> (),
	table_next_row: () -> (),
	table_next_column: () -> (),
	table_sort_specs: () -> SortSpec?,
	end_table: () -> (),
	begin_tab_bar: (id: string) -> boolean,
	begin_tab_item: (label: string) -> boolean,
	end_tab_item: () -> (),
	end_tab_bar: () -> (),
	begin_menu_bar: () -> boolean,
	end_menu_bar: () -> (),
	begin_menu: (label: string) -> boolean,
	end_menu: () -> (),
	menu_item: (label: string) -> boolean,
	open_popup: (id: string) -> (),
	begin_popup: (id: string) -> boolean,
	begin_popup_modal: (id: string, options: { [string]: any }?) -> boolean,
	begin_popup_context_item: (id: string) -> boolean,
	end_popup: () -> (),
	set_tooltip: (text: string) -> (),
	tree_node: (label: string) -> boolean,
	tree_pop: () -> (),
	collapsing_header: (label: string) -> boolean,
	begin_list_box: (id: string, size: Size?) -> boolean,
	end_list_box: () -> (),
	color_edit3: (label: string, colour: Rgb, options: { [string]: any }?) -> Rgb,
	image: (options: ImageOptions) -> ImageAnswer,
	scene3d: (options: SceneOptions) -> SceneAnswer,
	get_window_draw_list: () -> DrawList,
	draw: (list: DrawList) -> (),
	color_button: (options: { [string]: any }) -> boolean,
	begin_drag_drop_source: () -> boolean,
	set_drag_drop_payload: (kind: string, payload: any) -> (),
	end_drag_drop_source: () -> (),
	begin_drag_drop_target: () -> boolean,
	accept_drag_drop_payload: (kind: string) -> any?,
	end_drag_drop_target: () -> (),
	shortcut: (options: { [string]: any }) -> boolean,
	date_picker: (options: { [string]: any }) -> (),
	time_picker: (options: { [string]: any }) -> (),
}

export type ImGuiHandle = {
	on_render: (self: ImGuiHandle, callback: (ui: Ui) -> ()) -> (),
	on_close: (self: ImGuiHandle, callback: () -> ()) -> (),
	close: (self: ImGuiHandle) -> (),
	is_open: (self: ImGuiHandle) -> boolean,
	state: (self: ImGuiHandle) -> WindowState,
	show: (self: ImGuiHandle) -> (),
	hide: (self: ImGuiHandle) -> (),
	focus: (self: ImGuiHandle) -> (),
	minimize: (self: ImGuiHandle) -> (),
	restore: (self: ImGuiHandle) -> (),
	maximize: (self: ImGuiHandle) -> (),
	begin_drag: (self: ImGuiHandle) -> (),
	begin_resize: (self: ImGuiHandle, edge: string) -> (),
	is_maximized: (self: ImGuiHandle) -> boolean,
	set_title: (self: ImGuiHandle, title: string) -> (),
	set_size: (self: ImGuiHandle, size: Size) -> (),
}

export type ImGui = {
	open_window: (options: WindowOptions) -> ImGuiHandle,
	is_open: (window: string) -> boolean,
	draw_list: () -> DrawList,
	mesh: (options: { vertices: { number }, indices: { number } }) -> MeshHandle,
	texture: (options: { path: string }) -> TextureHandle,
	texture_size: (handle: TextureHandle) -> (number, number),
	font: (options: { path: string, size: number? }) -> FontHandle,
	font_size: (font: FontHandle) -> number,
	style: () -> ImGuiStyle,
	set_style: (options: StyleOptions) -> (),
	reset_style: () -> (),
}

export type Zeron = {
	imgui: ImGui,
}