//! Draw lists: the lines, shapes, text and pictures a plugin paints itself.
//!
//! Both lists a plugin can draw into end up here:
//!
//! * the frame's own, `ui.get_window_draw_list()`, whose calls paint at once
//!   into the window's Dear ImGui draw list;
//! * a plugin-held one, `imgui.draw_list()`, whose calls are kept in the
//!   script and replayed through `ui.draw(list)` — the same entry, so a
//!   mistake in a kept list is refused exactly as it would be in the frame's.
//!
//! Geometry is in window points, as every other widget is, and the clip a
//! command carries is applied around that one command: the script keeps its
//! clip stack, so a list kept across frames has no state on this side.
//!
//! Colours are `"#rrggbb"`, `"#rrggbbaa"` or a Zeron token (`"$accent"`),
//! resolved against the plugin's own style.

use dear_imgui_rs::Ui;
use zgx_script::Value;

use super::frame::type_name;
use super::style::Resolved;
use super::texture::PluginTextures;

/// One paint call: what to draw, and the clip in force where the script drew
/// it.
pub struct Paint<'a> {
    pub ui: &'a Ui,
    pub style: &'a Resolved,
    pub textures: &'a PluginTextures,
}

impl Paint<'_> {
    /// Applies one draw-list call.
    pub fn apply(&self, arg: &Value) -> Result<Value, String> {
        let kind = match arg.field("kind") {
            Some(Value::Str(kind)) => kind.to_string(),
            _ => return Err("a draw list call needs its kind".into()),
        };
        let clip = clip_of(arg)?;
        let draw_list = self.ui.get_window_draw_list();
        let _clip = clip.map(|(min, max)| draw_list.push_clip_rect(min, max, true));
        match kind.as_str() {
            "line" => draw_line(self, &draw_list, arg),
            "rect" => draw_rect(self, &draw_list, arg, false, false),
            "rect_filled" => draw_rect(self, &draw_list, arg, true, false),
            "rect_rounded" => draw_rect(self, &draw_list, arg, false, true),
            "rect_rounded_filled" => draw_rect(self, &draw_list, arg, true, true),
            "circle" => draw_circle(self, &draw_list, arg, false),
            "circle_filled" => draw_circle(self, &draw_list, arg, true),
            "text" => draw_text(self, &draw_list, arg),
            "image" => draw_image(self, &draw_list, arg),
            "poly_filled" => draw_poly(self, &draw_list, arg, true),
            "poly_line" => draw_poly(self, &draw_list, arg, false),
            other => Err(format!(
                "a draw list has no call \"{other}\": it draws lines, rectangles, circles, text, pictures and polygons."
            )),
        }
    }
}

/// A rectangle in points: its top-left and bottom-right corners.
type Rect = ([f32; 2], [f32; 2]);

/// The clip rectangle in force for this call, in points.
fn clip_of(arg: &Value) -> Result<Option<Rect>, String> {
    let Some(clip) = arg.field("clip") else {
        return Ok(None);
    };
    Ok(Some((
        point_of(clip, "min", "a clipped draw list call")?,
        point_of(clip, "max", "a clipped draw list call")?,
    )))
}

fn draw_line(
    paint: &Paint<'_>,
    draw_list: &dear_imgui_rs::DrawListMut<'_>,
    arg: &Value,
) -> Result<Value, String> {
    let from = point_of(arg, "from", "dl:add_line")?;
    let to = point_of(arg, "to", "dl:add_line")?;
    let colour = colour_of(paint.style, arg, "dl:add_line")?;
    let thickness = thickness_of(arg, "dl:add_line")?;
    draw_list
        .add_line(from, to, colour)
        .thickness(thickness)
        .build();
    Ok(drawn())
}

fn draw_rect(
    paint: &Paint<'_>,
    draw_list: &dear_imgui_rs::DrawListMut<'_>,
    arg: &Value,
    filled: bool,
    rounded: bool,
) -> Result<Value, String> {
    let op = match (filled, rounded) {
        (false, false) => "dl:add_rect",
        (true, false) => "dl:add_rect_filled",
        (false, true) => "dl:add_rect_rounded",
        (true, true) => "dl:add_rect_rounded_filled",
    };
    let min = point_of(arg, "min", op)?;
    let max = point_of(arg, "max", op)?;
    let colour = colour_of(paint.style, arg, op)?;
    let rounding = opt_number(arg, "rounding", op)?.unwrap_or(0.0);
    if !(0.0..=f64::from(f32::MAX)).contains(&rounding) {
        return Err(format!("{op} needs rounding to be zero or more."));
    }
    let thickness = thickness_of(arg, op)?;
    draw_list
        .add_rect(min, max, colour)
        .rounding(rounding as f32)
        .thickness(thickness)
        .filled(filled)
        .build();
    Ok(drawn())
}

fn draw_circle(
    paint: &Paint<'_>,
    draw_list: &dear_imgui_rs::DrawListMut<'_>,
    arg: &Value,
    filled: bool,
) -> Result<Value, String> {
    let op = if filled {
        "dl:add_circle_filled"
    } else {
        "dl:add_circle"
    };
    let center = point_of(arg, "center", op)?;
    let radius = match arg.field("radius") {
        Some(Value::Num(radius)) if radius.is_finite() && *radius > 0.0 => *radius as f32,
        Some(Value::Num(_)) => return Err(format!("{op} needs radius to be more than zero.")),
        Some(other) => {
            return Err(format!(
                "{op} needs radius to be a number, not {}.",
                type_name(other)
            ));
        }
        None => return Err(format!("{op} needs a radius, like {{ radius = 20 }}.")),
    };
    let colour = colour_of(paint.style, arg, op)?;
    let thickness = thickness_of(arg, op)?;
    draw_list
        .add_circle(center, radius, colour)
        .thickness(thickness)
        .filled(filled)
        .build();
    Ok(drawn())
}

fn draw_text(
    paint: &Paint<'_>,
    draw_list: &dear_imgui_rs::DrawListMut<'_>,
    arg: &Value,
) -> Result<Value, String> {
    let pos = point_of(arg, "pos", "dl:add_text")?;
    let colour = colour_of(paint.style, arg, "dl:add_text")?;
    let text = match arg.field("text") {
        Some(Value::Str(text)) => text.to_string(),
        Some(Value::Text(text)) => text.to_string(),
        Some(other) => {
            return Err(format!(
                "dl:add_text needs text to be a string, not {}.",
                type_name(other)
            ));
        }
        None => return Err("dl:add_text needs text, like { text = \"Hello\" }.".into()),
    };
    let size = opt_number(arg, "font_size", "dl:add_text")?.unwrap_or(0.0);
    if size < 0.0 {
        return Err("dl:add_text needs font_size to be zero or more.".into());
    }
    let font = paint.ui.current_font();
    draw_list.add_text_with_font(font, size as f32, pos, colour, &text, 0.0, None);
    Ok(drawn())
}

fn draw_image(
    paint: &Paint<'_>,
    draw_list: &dear_imgui_rs::DrawListMut<'_>,
    arg: &Value,
) -> Result<Value, String> {
    let texture = texture_of(paint.textures, arg)?;
    let min = point_of(arg, "min", "dl:add_image")?;
    let max = point_of(arg, "max", "dl:add_image")?;
    let uv_min = opt_point(arg, "uv_min")?.unwrap_or([0.0, 0.0]);
    let uv_max = opt_point(arg, "uv_max")?.unwrap_or([1.0, 1.0]);
    let tint = match arg.field("color").or_else(|| arg.field("colour")) {
        None => [1.0, 1.0, 1.0, 1.0],
        Some(_) => colour_of(paint.style, arg, "dl:add_image")?,
    };
    draw_list.add_image(
        dear_imgui_rs::TextureId::new(texture.id),
        min,
        max,
        uv_min,
        uv_max,
        tint,
    );
    Ok(drawn())
}

fn draw_poly(
    paint: &Paint<'_>,
    draw_list: &dear_imgui_rs::DrawListMut<'_>,
    arg: &Value,
    filled: bool,
) -> Result<Value, String> {
    let op = if filled {
        "dl:add_poly_filled"
    } else {
        "dl:add_poly_line"
    };
    let Some(Value::List(points)) = arg.field("points") else {
        return Err(format!(
            "{op} needs points, like {{ points = {{ {{ 0, 0 }}, {{ 10, 0 }}, {{ 5, 10 }} }} }}."
        ));
    };
    if points.len() < 3 {
        return Err(format!("{op} needs at least three points."));
    }
    let mut corners = Vec::with_capacity(points.len());
    for point in points.iter() {
        corners.push(point_value(point, op)?);
    }
    let colour = colour_of(paint.style, arg, op)?;
    let thickness = thickness_of(arg, op)?;
    let closed = match arg.field("closed") {
        None => true,
        Some(value) => value.truthy(),
    };
    draw_list
        .add_polyline(corners, colour)
        .thickness(thickness)
        .closed(closed)
        .filled(filled)
        .build();
    Ok(drawn())
}

fn drawn() -> Value {
    Value::record([("ok", Value::Bool(true))])
}

/// A `{ x, y }` map or a `{ x, y }` list, as points.
fn point_of(arg: &Value, field: &str, op: &str) -> Result<[f32; 2], String> {
    let Some(value) = arg.field(field) else {
        return Err(format!(
            "{op} needs {field}, like {{ {field} = {{ 10, 20 }} }}."
        ));
    };
    point_value(value, op)
}

fn point_value(value: &Value, op: &str) -> Result<[f32; 2], String> {
    let pair = match value {
        Value::List(items) if items.len() >= 2 => Some((&items[0], &items[1])),
        Value::Map(_) => Some((
            value.field("x").unwrap_or(&Value::Nil),
            value.field("y").unwrap_or(&Value::Nil),
        )),
        _ => None,
    };
    let read = |value: &Value| match value {
        Value::Num(number) if number.is_finite() => Some(*number as f32),
        _ => None,
    };
    match pair {
        Some((x, y)) => match (read(x), read(y)) {
            (Some(x), Some(y)) => Ok([x, y]),
            _ => Err(format!(
                "{op} needs a point of two numbers, like {{ 10, 20 }}."
            )),
        },
        None => Err(format!(
            "{op} needs a point of two numbers, like {{ 10, 20 }}."
        )),
    }
}

fn opt_point(arg: &Value, field: &str) -> Result<Option<[f32; 2]>, String> {
    match arg.field(field) {
        None => Ok(None),
        Some(value) => point_value(value, "a draw list call").map(Some),
    }
}

fn opt_number(arg: &Value, field: &str, op: &str) -> Result<Option<f64>, String> {
    match arg.field(field) {
        None => Ok(None),
        Some(Value::Num(number)) if number.is_finite() => Ok(Some(*number)),
        Some(Value::Num(_)) => Err(format!("{op} needs {field} to be a finite number.")),
        Some(other) => Err(format!(
            "{op} needs {field} to be a number, not {}.",
            type_name(other)
        )),
    }
}

fn thickness_of(arg: &Value, op: &str) -> Result<f32, String> {
    match opt_number(arg, "thickness", op)? {
        None => Ok(1.0),
        Some(value) if value > 0.0 && value <= 64.0 => Ok(value as f32),
        Some(_) => Err(format!(
            "{op} needs thickness to be more than zero and at most 64 points."
        )),
    }
}

/// The colour a call names: a hex string, or one of the plugin's tokens.
fn colour_of(style: &Resolved, arg: &Value, op: &str) -> Result<[f32; 4], String> {
    let Some(value) = arg.field("color").or_else(|| arg.field("colour")) else {
        return Err(format!(
            "{op} needs a colour, like {{ color = \"#ff0000\" }} or {{ color = \"$accent\" }}."
        ));
    };
    let text = match value {
        Value::Str(text) => text.to_string(),
        Value::Text(text) => text.to_string(),
        other => {
            return Err(format!(
                "{op} needs color to be a colour string, not {}.",
                type_name(other)
            ));
        }
    };
    style.colour(&text).ok_or_else(|| {
        if text.starts_with('$') {
            format!(
                "{op} can't use colour \"{text}\": the tokens are {}.",
                super::style::TOKENS.join(", ")
            )
        } else {
            format!(
                "{op} can't use colour \"{text}\": colour must be \"#rrggbb\", \"#rrggbbaa\" or a token like \"$accent\"."
            )
        }
    })
}

/// The picture a call names: a handle this plugin's `imgui.texture` made.
fn texture_of(
    textures: &PluginTextures,
    arg: &Value,
) -> Result<std::rc::Rc<super::texture::TextureImage>, String> {
    let Some(value) = arg.field("handle") else {
        return Err(
            "dl:add_image needs a handle, like { handle = imgui.texture{ path = … } }.".into(),
        );
    };
    let id = match value {
        Value::Num(id) if id.is_finite() && *id >= 0.0 => *id as u64,
        other => {
            return Err(format!(
                "dl:add_image needs handle to be a texture handle from imgui.texture, not {}.",
                type_name(other)
            ));
        }
    };
    textures.get(id).ok_or_else(|| {
        "dl:add_image needs handle to be a texture handle this plugin made with imgui.texture{ path = … }.".to_string()
    })
}
