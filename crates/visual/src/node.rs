//! The element tree a visual's view function returns, parsed from the tagged
//! tables `ui.*` constructors build. The tree is plain data: the desktop
//! renders it with native elements, and handlers are indexes into the
//! render's handler list.

use mlua::{Function, Table, Value};

use crate::VisualError;
use crate::style::{Align, Color, Justify, TextSize, Token, Weight};

/// A Lua function a node calls back (an event handler or a draw function),
/// valid until the next render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HandlerId(pub u32);

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub kind: NodeKind,
    /// Fixed width in design pixels, inside a row or grid.
    pub width: Option<f32>,
    /// Takes the row's spare width.
    pub grow: bool,
    /// The script's stable name for this node, so view-local state (a
    /// chart's hover, a scene's camera) survives the tree changing shape.
    pub key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Axis {
    #[default]
    Vertical,
    Horizontal,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    Stack(Stack),
    Grid {
        columns: u16,
        gap: f32,
        children: Vec<Node>,
    },
    Text(Text),
    /// A hairline divider.
    Rule,
    Space(f32),
    /// A headline figure over a short label.
    Stat {
        value: String,
        label: String,
        /// A short change note beside the figure ("+12%").
        delta: Option<String>,
        tone: Option<Color>,
    },
    Table(Table_),
    Chart(Box<Chart>),
    Canvas(Canvas),
    Scene(SceneNode),
    Shader(ShaderNode),
    Button {
        label: String,
        on_click: HandlerId,
        primary: bool,
    },
    Tabs {
        options: Vec<String>,
        value: String,
        on_change: HandlerId,
    },
    Toggle {
        label: String,
        value: bool,
        on_change: HandlerId,
    },
    Slider {
        label: Option<String>,
        value: f64,
        min: f64,
        max: f64,
        step: f64,
        on_change: HandlerId,
        format: NumberFormat,
    },
    Link {
        label: String,
        url: String,
    },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Stack {
    pub axis: Axis,
    pub gap: f32,
    pub align: Align,
    pub justify: Justify,
    pub wrap: bool,
    /// The hover wash, the only fill a visual has: for rows the reader can
    /// click or scan.
    pub hover: bool,
    pub on_click: Option<HandlerId>,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Text {
    pub spans: Vec<Span>,
    pub size: TextSize,
    pub weight: Weight,
    pub color: Option<Color>,
    pub mono: bool,
    pub italic: bool,
    pub align: Align,
    pub truncate: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Span {
    pub text: String,
    pub weight: Option<Weight>,
    pub color: Option<Color>,
    pub mono: bool,
    pub italic: bool,
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Table_ {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Node>>,
    /// Rows take the hover wash.
    pub hover: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Column {
    pub label: String,
    pub align: Align,
    pub width: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NumberFormat {
    pub prefix: String,
    pub unit: String,
    pub decimals: Option<u8>,
}

impl NumberFormat {
    pub fn format(&self, value: f64) -> String {
        let decimals = self.decimals.map(usize::from).unwrap_or_else(|| {
            let magnitude = value.abs();
            if magnitude == 0.0 || magnitude >= 100.0 || value.fract() == 0.0 {
                0
            } else if magnitude >= 10.0 {
                1
            } else {
                2
            }
        });
        let text = format!("{value:.decimals$}");
        let (sign, digits) = match text.strip_prefix('-') {
            Some(rest) => ("-", rest),
            None => ("", text.as_str()),
        };
        let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
        let mut grouped = String::new();
        for (ix, ch) in int.chars().enumerate() {
            if ix > 0 && (int.len() - ix) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(ch);
        }
        let frac = if frac.is_empty() {
            String::new()
        } else {
            format!(".{frac}")
        };
        let space = if self.unit.is_empty() || self.unit.starts_with('%') {
            ""
        } else {
            " "
        };
        format!("{sign}{}{grouped}{frac}{space}{}", self.prefix, self.unit)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartKind {
    Bar,
    Line,
    Area,
    Scatter,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chart {
    pub kind: ChartKind,
    pub height: f32,
    /// Category labels (bar, line, area), one per value.
    pub x: Vec<String>,
    pub series: Vec<Series>,
    pub stacked: bool,
    pub horizontal: bool,
    pub y_min: Option<f64>,
    pub y_max: Option<f64>,
    pub format: NumberFormat,
    /// Scatter x axis format.
    pub x_format: NumberFormat,
    pub legend: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub name: String,
    pub color: Color,
    /// One value per category; `nil` leaves a gap.
    pub values: Vec<Option<f64>>,
    /// Scatter points.
    pub points: Vec<(f64, f64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Canvas {
    pub height: f32,
    pub draw: HandlerId,
    pub animate: bool,
    pub on_click: Option<HandlerId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneNode {
    pub height: f32,
    pub draw: HandlerId,
    pub animate: bool,
    pub orbit: bool,
    /// Turntable speed in radians per second while the reader isn't dragging.
    pub spin: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
}

/// A fragment shader the desktop runs on the GPU (see [`crate::shader`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ShaderNode {
    pub height: f32,
    /// GLSL defining `mainImage`: the image pass.
    pub code: String,
    /// Buffer passes, run in order before the image (`iChannel0`-`3`).
    pub buffers: Vec<String>,
    /// Named number inputs, in slot order.
    pub inputs: Vec<(String, f32)>,
    /// Advance `iTime` every frame (on by default).
    pub animate: bool,
    /// Dragging orbits `iCamera` (otherwise it only moves `iMouse`).
    pub orbit: bool,
    pub spin: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
    /// How the output sits on the chat.
    pub blend: crate::shader::Blend,
    /// Bloom strength on bright output (0 off).
    pub bloom: f32,
    /// Image assets every pass samples as `iTexture0`-`3`.
    pub textures: Vec<String>,
}

pub const MIN_HEIGHT: f32 = 40.0;
pub const MAX_HEIGHT: f32 = 720.0;
const MAX_NODES: usize = 4000;
const MAX_DEPTH: usize = 48;
const MAX_TEXT: usize = 8000;

/// Builds a [`Node`] tree from a view's return value, collecting the
/// functions it references.
pub(crate) struct Parser {
    pub handlers: Vec<Function>,
    nodes: usize,
}

/// Field names every node accepts.
const COMMON: [&str; 4] = ["__kind", "width", "grow", "key"];

/// Props that exist in web styling but not here, with the reason, so the
/// agent learns the rule from the error.
fn forbidden(prop: &str) -> Option<&'static str> {
    Some(match prop {
        "background" | "bg" | "fill" | "shadow" | "border" | "padding" | "radius" | "blur"
        | "opacity" => {
            "visuals have no containers, fills, borders, or shadows: lay content directly on \
             the chat with gap, ui.rule(), and type"
        }
        "font" | "family" | "font_family" | "fontFamily" | "font_size" | "fontSize" => {
            "text always renders in Zeron's interface font; size it with size = xs, sm, body, \
             lg, xl, or 2xl"
        }
        _ => return None,
    })
}

impl Parser {
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
            nodes: 0,
        }
    }

    fn handler(&mut self, function: Function) -> HandlerId {
        self.handlers.push(function);
        HandlerId(self.handlers.len() as u32 - 1)
    }

    pub fn root(&mut self, value: Value) -> Result<Node, VisualError> {
        let mut children = Vec::new();
        self.children_into(value, &mut children, 0)?;
        Ok(match children.len() {
            1 => children.pop().unwrap(),
            _ => Node {
                kind: NodeKind::Stack(Stack {
                    gap: 16.0,
                    children,
                    ..Default::default()
                }),
                width: None,
                grow: false,
                key: None,
            },
        })
    }

    /// Children may be nodes, strings (plain text), nested lists of either
    /// (from a map), or nil/false (conditional rendering).
    fn children_into(
        &mut self,
        value: Value,
        out: &mut Vec<Node>,
        depth: usize,
    ) -> Result<(), VisualError> {
        if depth > MAX_DEPTH {
            return Err(VisualError::script("the view nests too deeply"));
        }
        match value {
            Value::Nil | Value::Boolean(false) => {}
            Value::String(s) => out.push(self.plain_text(s.to_string_lossy())?),
            Value::Integer(_) | Value::Number(_) => out.push(self.plain_text(number_text(&value))?),
            Value::Table(table) => {
                if table.raw_get::<Option<String>>("__kind")?.is_some() {
                    out.push(self.node(&table, depth)?);
                } else {
                    for child in table.sequence_values::<Value>() {
                        self.children_into(child?, out, depth + 1)?;
                    }
                }
            }
            other => {
                return Err(VisualError::script(format!(
                    "a view child must be a ui node or a string, not a {}",
                    other.type_name()
                )));
            }
        }
        Ok(())
    }

    fn count(&mut self) -> Result<(), VisualError> {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(VisualError::script(format!(
                "the view has more than {MAX_NODES} nodes; summarize or paginate the data"
            )));
        }
        Ok(())
    }

    fn plain_text(&mut self, text: String) -> Result<Node, VisualError> {
        self.count()?;
        Ok(Node {
            kind: NodeKind::Text(Text {
                spans: vec![Span {
                    text: clip(text),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            width: None,
            grow: false,
            key: None,
        })
    }

    fn node(&mut self, table: &Table, depth: usize) -> Result<Node, VisualError> {
        self.count()?;
        let kind: String = table.raw_get("__kind")?;
        let props = Props { table, kind: &kind };
        let node_kind = match kind.as_str() {
            "column" | "row" => {
                props.allow(&["gap", "align", "justify", "wrap", "hover", "on_click"])?;
                let mut children = Vec::new();
                for child in table.sequence_values::<Value>() {
                    self.children_into(child?, &mut children, depth + 1)?;
                }
                NodeKind::Stack(Stack {
                    axis: if kind == "row" {
                        Axis::Horizontal
                    } else {
                        Axis::Vertical
                    },
                    gap: props
                        .number("gap")?
                        .unwrap_or(if kind == "row" { 12.0 } else { 8.0 })
                        .clamp(0.0, 96.0) as f32,
                    align: props.parsed("align", Align::parse)?.unwrap_or_default(),
                    justify: props.parsed("justify", Justify::parse)?.unwrap_or_default(),
                    wrap: props.bool("wrap")?.unwrap_or(false),
                    hover: props.bool("hover")?.unwrap_or(false),
                    on_click: props.function("on_click")?.map(|f| self.handler(f)),
                    children,
                })
            }
            "grid" => {
                props.allow(&["columns", "gap"])?;
                let mut children = Vec::new();
                for child in table.sequence_values::<Value>() {
                    self.children_into(child?, &mut children, depth + 1)?;
                }
                NodeKind::Grid {
                    columns: props.number("columns")?.unwrap_or(3.0).clamp(1.0, 12.0) as u16,
                    gap: props.number("gap")?.unwrap_or(16.0).clamp(0.0, 96.0) as f32,
                    children,
                }
            }
            "text" => NodeKind::Text(self.text(&props)?),
            "rule" => {
                props.allow(&[])?;
                NodeKind::Rule
            }
            "space" => {
                props.allow(&["size"])?;
                NodeKind::Space(props.number("size")?.unwrap_or(16.0).clamp(0.0, 200.0) as f32)
            }
            "stat" => {
                props.allow(&["value", "label", "delta", "tone"])?;
                NodeKind::Stat {
                    value: props.text_value("value")?.unwrap_or_default(),
                    label: props.string("label")?.unwrap_or_default(),
                    delta: props.text_value("delta")?,
                    tone: props.color("tone")?,
                }
            }
            "table" => NodeKind::Table(self.table(&props, depth)?),
            "chart" => NodeKind::Chart(Box::new(chart(&props)?)),
            "canvas" => {
                props.allow(&["height", "draw", "animate", "on_click"])?;
                NodeKind::Canvas(Canvas {
                    height: props.height()?,
                    draw: self.required_function(&props, "draw")?,
                    animate: props.bool("animate")?.unwrap_or(false),
                    on_click: props.function("on_click")?.map(|f| self.handler(f)),
                })
            }
            "scene" => {
                props.allow(&["height", "draw", "animate", "orbit", "spin", "camera"])?;
                let camera: Option<Table> = props.get("camera")?;
                let (mut yaw, mut pitch, mut zoom) = (0.6, 0.45, 1.0);
                if let Some(camera) = camera {
                    yaw = camera.get::<Option<f64>>("yaw")?.unwrap_or(yaw);
                    pitch = camera.get::<Option<f64>>("pitch")?.unwrap_or(pitch);
                    zoom = camera.get::<Option<f64>>("zoom")?.unwrap_or(zoom);
                }
                NodeKind::Scene(SceneNode {
                    height: props.height()?,
                    draw: self.required_function(&props, "draw")?,
                    animate: props.bool("animate")?.unwrap_or(false),
                    orbit: props.bool("orbit")?.unwrap_or(true),
                    spin: props.number("spin")?.unwrap_or(0.0).clamp(-3.0, 3.0) as f32,
                    yaw: yaw as f32,
                    pitch: (pitch as f32).clamp(-1.5, 1.5),
                    zoom: (zoom as f32).clamp(0.2, 5.0),
                })
            }
            "shader" => {
                props.allow(&[
                    "height",
                    "code",
                    "inputs",
                    "animate",
                    "orbit",
                    "spin",
                    "camera",
                    "blend",
                    "bloom",
                    "buffers",
                    "textures",
                    // Before `blend`: true kept the alpha, false was opaque.
                    "transparent",
                ])?;
                let code = match props.table.raw_get::<Value>("code")? {
                    Value::String(code) => code.to_string_lossy(),
                    Value::Nil => {
                        return Err(props.error(
                            "needs code: GLSL defining void mainImage(out vec4 fragColor, in \
                             vec2 fragCoord)",
                        ));
                    }
                    _ => return Err(props.error("'code' must be a string of GLSL")),
                };
                let mut buffers = Vec::new();
                if let Some(table) = props.get::<Option<Table>>("buffers")? {
                    for value in table.sequence_values::<Value>() {
                        match value? {
                            Value::String(code) => buffers.push(code.to_string_lossy()),
                            _ => return Err(props.error("'buffers' is a list of GLSL strings")),
                        }
                    }
                }
                if buffers.len() > crate::shader::MAX_BUFFERS {
                    return Err(props.error(&format!(
                        "takes at most {} buffers",
                        crate::shader::MAX_BUFFERS
                    )));
                }
                let mut textures = Vec::new();
                if let Some(table) = props.get::<Option<Table>>("textures")? {
                    for value in table.sequence_values::<Value>() {
                        match value? {
                            Value::String(name) => textures.push(name.to_string_lossy()),
                            _ => return Err(props.error("'textures' is a list of asset names")),
                        }
                    }
                }
                if textures.len() > crate::shader::MAX_TEXTURES {
                    return Err(props.error(&format!(
                        "takes at most {} textures",
                        crate::shader::MAX_TEXTURES
                    )));
                }
                let blend = match (props.string("blend")?, props.bool("transparent")?) {
                    (Some(name), _) => crate::shader::Blend::parse(&name).ok_or_else(|| {
                        props.error("'blend' is \"alpha\", \"light\", or \"opaque\"")
                    })?,
                    (None, Some(true)) => crate::shader::Blend::Alpha,
                    (None, Some(false)) => crate::shader::Blend::Opaque,
                    (None, None) => crate::shader::Blend::default(),
                };
                let mut inputs = Vec::new();
                if let Some(table) = props.get::<Option<Table>>("inputs")? {
                    for pair in table.pairs::<String, f64>() {
                        let (name, value) =
                            pair.map_err(|_| props.error("'inputs' maps names to numbers"))?;
                        if !crate::shader::valid_input_name(&name) {
                            return Err(
                                props.error(&format!("input '{name}' is not a usable GLSL name"))
                            );
                        }
                        inputs.push((name, value as f32));
                    }
                }
                if inputs.len() > crate::shader::MAX_INPUTS {
                    return Err(props.error(&format!(
                        "takes at most {} inputs",
                        crate::shader::MAX_INPUTS
                    )));
                }
                // Slots follow the names, so a shader compiled for one set of
                // values serves every other.
                inputs.sort_by(|a, b| a.0.cmp(&b.0));
                let camera: Option<Table> = props.get("camera")?;
                let (mut yaw, mut pitch, mut zoom) = (0.0, 0.0, 1.0);
                if let Some(camera) = camera {
                    yaw = camera.get::<Option<f64>>("yaw")?.unwrap_or(yaw);
                    pitch = camera.get::<Option<f64>>("pitch")?.unwrap_or(pitch);
                    zoom = camera.get::<Option<f64>>("zoom")?.unwrap_or(zoom);
                }
                NodeKind::Shader(ShaderNode {
                    height: props.height()?,
                    code,
                    buffers,
                    inputs,
                    animate: props.bool("animate")?.unwrap_or(true),
                    orbit: props.bool("orbit")?.unwrap_or(false),
                    spin: props.number("spin")?.unwrap_or(0.0).clamp(-3.0, 3.0) as f32,
                    yaw: yaw as f32,
                    pitch: (pitch as f32).clamp(-1.5, 1.5),
                    zoom: (zoom as f32).clamp(0.2, 5.0),
                    blend,
                    bloom: props.number("bloom")?.unwrap_or(0.0).clamp(0.0, 3.0) as f32,
                    textures,
                })
            }
            "button" => {
                props.allow(&["label", "on_click", "primary"])?;
                NodeKind::Button {
                    label: props.required_string("label")?,
                    on_click: self.required_function(&props, "on_click")?,
                    primary: props.bool("primary")?.unwrap_or(false),
                }
            }
            "tabs" => {
                props.allow(&["options", "value", "on_change"])?;
                let options: Vec<String> = props
                    .get::<Option<Vec<Value>>>("options")?
                    .unwrap_or_default()
                    .iter()
                    .map(value_text)
                    .collect();
                if options.is_empty() {
                    return Err(props.error("needs options = { ... }"));
                }
                let value = props
                    .text_value("value")?
                    .unwrap_or_else(|| options[0].clone());
                NodeKind::Tabs {
                    options,
                    value,
                    on_change: self.required_function(&props, "on_change")?,
                }
            }
            "toggle" => {
                props.allow(&["label", "value", "on_change"])?;
                NodeKind::Toggle {
                    label: props.string("label")?.unwrap_or_default(),
                    value: props.bool("value")?.unwrap_or(false),
                    on_change: self.required_function(&props, "on_change")?,
                }
            }
            "slider" => {
                props.allow(&[
                    "label",
                    "value",
                    "min",
                    "max",
                    "step",
                    "on_change",
                    "unit",
                    "prefix",
                    "decimals",
                ])?;
                let min = props.number("min")?.unwrap_or(0.0);
                let max = props.number("max")?.unwrap_or(1.0).max(min + f64::EPSILON);
                NodeKind::Slider {
                    label: props.string("label")?,
                    value: props.number("value")?.unwrap_or(min).clamp(min, max),
                    min,
                    max,
                    step: props.number("step")?.unwrap_or(0.0).max(0.0),
                    on_change: self.required_function(&props, "on_change")?,
                    format: props.format()?,
                }
            }
            "link" => {
                props.allow(&["label", "url"])?;
                let url = props.required_string("url")?;
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    return Err(props.error("url must be http(s)"));
                }
                NodeKind::Link {
                    label: props.string("label")?.unwrap_or_else(|| url.clone()),
                    url,
                }
            }
            "span" => {
                return Err(VisualError::script(
                    "ui.span goes inside ui.text { ... }, not directly in a layout",
                ));
            }
            other => return Err(VisualError::script(format!("unknown node '{other}'"))),
        };
        Ok(Node {
            kind: node_kind,
            width: props.number("width")?.map(|w| w.clamp(0.0, 2000.0) as f32),
            grow: props.bool("grow")?.unwrap_or(false),
            key: props.string("key")?,
        })
    }

    fn required_function(&mut self, props: &Props, name: &str) -> Result<HandlerId, VisualError> {
        match props.function(name)? {
            Some(function) => Ok(self.handler(function)),
            None => Err(props.error(&format!("needs {name} = function(...) end"))),
        }
    }

    fn text(&mut self, props: &Props) -> Result<Text, VisualError> {
        props.allow(&[
            "size", "weight", "color", "mono", "italic", "align", "truncate",
        ])?;
        let mut spans = Vec::new();
        let mut len = 0usize;
        for child in props.table.sequence_values::<Value>() {
            let span = match child? {
                Value::Table(table)
                    if table.raw_get::<Option<String>>("__kind")?.as_deref() == Some("span") =>
                {
                    let span_props = Props {
                        table: &table,
                        kind: "span",
                    };
                    span_props.allow(&["weight", "color", "mono", "italic"])?;
                    let text: String = table
                        .sequence_values::<Value>()
                        .map(|v| v.map(|v| value_text(&v)))
                        .collect::<Result<Vec<_>, _>>()?
                        .concat();
                    Span {
                        text,
                        weight: span_props.weight()?,
                        color: span_props.color("color")?,
                        mono: span_props.bool("mono")?.unwrap_or(false),
                        italic: span_props.bool("italic")?.unwrap_or(false),
                    }
                }
                Value::Table(_) => {
                    return Err(props.error("children must be strings or ui.span { ... }"));
                }
                Value::Nil | Value::Boolean(false) => continue,
                value => Span {
                    text: value_text(&value),
                    ..Default::default()
                },
            };
            len += span.text.len();
            spans.push(span);
        }
        if len > MAX_TEXT {
            return Err(props.error(&format!("is over {MAX_TEXT} characters")));
        }
        Ok(Text {
            spans,
            size: props.parsed("size", TextSize::parse)?.unwrap_or_default(),
            weight: props.weight()?.unwrap_or_default(),
            color: props.color("color")?,
            mono: props.bool("mono")?.unwrap_or(false),
            italic: props.bool("italic")?.unwrap_or(false),
            align: props.parsed("align", Align::parse)?.unwrap_or_default(),
            truncate: props.bool("truncate")?.unwrap_or(false),
        })
    }

    fn table(&mut self, props: &Props, depth: usize) -> Result<Table_, VisualError> {
        props.allow(&["columns", "rows", "hover"])?;
        let mut columns = Vec::new();
        for column in props
            .get::<Option<Vec<Value>>>("columns")?
            .unwrap_or_default()
        {
            columns.push(match column {
                Value::Table(t) => Column {
                    label: t.get::<Option<String>>("label")?.unwrap_or_default(),
                    align: match t.get::<Option<String>>("align")? {
                        Some(a) => Align::parse(&a)?,
                        None => Align::Start,
                    },
                    width: t.get::<Option<f64>>("width")?.map(|w| w as f32),
                },
                other => Column {
                    label: value_text(&other),
                    ..Default::default()
                },
            });
        }
        let mut rows = Vec::new();
        for row in props.get::<Option<Vec<Table>>>("rows")?.unwrap_or_default() {
            let mut cells = Vec::new();
            for cell in row.sequence_values::<Value>() {
                let cell = cell?;
                let numeric = matches!(cell, Value::Integer(_) | Value::Number(_));
                let mut out = Vec::new();
                self.children_into(cell, &mut out, depth + 1)?;
                let node = match out.len() {
                    0 => self.plain_text(String::new())?,
                    1 => out.pop().unwrap(),
                    _ => Node {
                        kind: NodeKind::Stack(Stack {
                            axis: Axis::Horizontal,
                            gap: 6.0,
                            children: out,
                            ..Default::default()
                        }),
                        width: None,
                        grow: false,
                        key: None,
                    },
                };
                // Numbers right-align when the script didn't say otherwise.
                if numeric
                    && let Some(column) = columns.get_mut(cells.len())
                    && column.align == Align::Start
                {
                    column.align = Align::End;
                }
                cells.push(node);
            }
            rows.push(cells);
        }
        if rows.len() > 500 {
            return Err(props.error("has more than 500 rows; summarize or paginate"));
        }
        Ok(Table_ {
            columns,
            rows,
            hover: props.bool("hover")?.unwrap_or(true),
        })
    }
}

fn chart(props: &Props) -> Result<Chart, VisualError> {
    props.allow(&[
        "type",
        "height",
        "x",
        "series",
        "stacked",
        "horizontal",
        "min",
        "max",
        "unit",
        "prefix",
        "decimals",
        "x_unit",
        "x_decimals",
        "legend",
    ])?;
    let kind = match props.string("type")?.as_deref().unwrap_or("bar") {
        "bar" => ChartKind::Bar,
        "line" => ChartKind::Line,
        "area" => ChartKind::Area,
        "scatter" => ChartKind::Scatter,
        other => {
            return Err(props.error(&format!(
                "has unknown type '{other}': use bar, line, area, or scatter (draw anything else \
                 with ui.canvas)"
            )));
        }
    };
    let x: Vec<String> = props
        .get::<Option<Vec<Value>>>("x")?
        .unwrap_or_default()
        .iter()
        .map(value_text)
        .collect();
    let mut series = Vec::new();
    for (ix, s) in props
        .get::<Option<Vec<Table>>>("series")?
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        let s_props = Props {
            table: &s,
            kind: "chart series",
        };
        s_props.allow(&["name", "values", "points", "color"])?;
        let values = match s.get::<Option<Table>>("values")? {
            Some(values) => {
                let len = values.raw_len();
                (1..=len)
                    .map(|i| values.raw_get::<Option<f64>>(i))
                    .collect::<Result<Vec<_>, _>>()?
            }
            None => Vec::new(),
        };
        let mut points = Vec::new();
        for point in s.get::<Option<Vec<Table>>>("points")?.unwrap_or_default() {
            let px: f64 = point.raw_get(1).or_else(|_| point.get("x"))?;
            let py: f64 = point.raw_get(2).or_else(|_| point.get("y"))?;
            points.push((px, py));
        }
        if values.len() + points.len() > 20_000 {
            return Err(props.error("has more than 20,000 values in a series"));
        }
        series.push(Series {
            name: s_props.string("name")?.unwrap_or_default(),
            color: s_props
                .color("color")?
                .unwrap_or(Color::token(Token::series(ix))),
            values,
            points,
        });
    }
    if series.is_empty() {
        return Err(props.error("needs series = { { name = ..., values = { ... } } }"));
    }
    if kind == ChartKind::Scatter && series.iter().all(|s| s.points.is_empty()) {
        return Err(props.error("of type scatter needs series with points = { {x, y}, ... }"));
    }
    if kind != ChartKind::Scatter {
        let longest = series.iter().map(|s| s.values.len()).max().unwrap_or(0);
        if longest == 0 {
            return Err(props.error("needs values in its series"));
        }
        if x.len() > 2000 || longest > 2000 {
            return Err(props.error("has more than 2,000 categories"));
        }
    }
    let format = props.format()?;
    Ok(Chart {
        kind,
        height: props.number("height")?.unwrap_or(220.0).clamp(80.0, 600.0) as f32,
        x,
        series,
        stacked: props.bool("stacked")?.unwrap_or(false),
        horizontal: props.bool("horizontal")?.unwrap_or(false),
        y_min: props.number("min")?,
        y_max: props.number("max")?,
        format,
        x_format: NumberFormat {
            prefix: String::new(),
            unit: props.string("x_unit")?.unwrap_or_default(),
            decimals: props.number("x_decimals")?.map(|d| d.clamp(0.0, 6.0) as u8),
        },
        legend: props.bool("legend")?.unwrap_or(true),
    })
}

struct Props<'a> {
    table: &'a Table,
    kind: &'a str,
}

impl Props<'_> {
    fn error(&self, message: &str) -> VisualError {
        VisualError::script(format!("ui.{} {message}", self.kind))
    }

    /// Reject props the node doesn't know, so a typo or a web-style prop
    /// fails at `show_visual` time instead of being silently ignored.
    fn allow(&self, known: &[&str]) -> Result<(), VisualError> {
        for pair in self.table.pairs::<Value, Value>() {
            let (key, _) = pair?;
            let Value::String(key) = key else { continue };
            let key = key.to_string_lossy();
            if COMMON.contains(&key.as_str()) || known.contains(&key.as_str()) {
                continue;
            }
            let reason = forbidden(&key)
                .map(|r| format!(": {r}"))
                .unwrap_or_else(|| {
                    if known.is_empty() {
                        String::new()
                    } else {
                        format!(" (it takes {})", known.join(", "))
                    }
                });
            return Err(self.error(&format!("has no '{key}'{reason}")));
        }
        Ok(())
    }

    fn get<T: mlua::FromLua>(&self, name: &str) -> Result<T, VisualError> {
        self.table
            .raw_get(name)
            .map_err(|e| self.error(&format!("'{name}': {}", short_error(&e))))
    }

    fn number(&self, name: &str) -> Result<Option<f64>, VisualError> {
        self.get(name)
    }

    fn bool(&self, name: &str) -> Result<Option<bool>, VisualError> {
        match self.table.raw_get::<Value>(name)? {
            Value::Nil => Ok(None),
            Value::Boolean(b) => Ok(Some(b)),
            _ => Err(self.error(&format!("'{name}' must be true or false"))),
        }
    }

    fn string(&self, name: &str) -> Result<Option<String>, VisualError> {
        Ok(match self.table.raw_get::<Value>(name)? {
            Value::Nil => None,
            value @ (Value::String(_) | Value::Integer(_) | Value::Number(_)) => {
                Some(clip(value_text(&value)))
            }
            _ => return Err(self.error(&format!("'{name}' must be a string"))),
        })
    }

    fn required_string(&self, name: &str) -> Result<String, VisualError> {
        self.string(name)?
            .ok_or_else(|| self.error(&format!("needs {name}")))
    }

    /// A string or a number (formatted as Lua prints it).
    fn text_value(&self, name: &str) -> Result<Option<String>, VisualError> {
        self.string(name)
    }

    fn function(&self, name: &str) -> Result<Option<Function>, VisualError> {
        match self.table.raw_get::<Value>(name)? {
            Value::Nil => Ok(None),
            Value::Function(f) => Ok(Some(f)),
            _ => Err(self.error(&format!("'{name}' must be a function"))),
        }
    }

    fn parsed<T>(
        &self,
        name: &str,
        parse: fn(&str) -> Result<T, VisualError>,
    ) -> Result<Option<T>, VisualError> {
        self.string(name)?.map(|s| parse(&s)).transpose()
    }

    fn weight(&self) -> Result<Option<Weight>, VisualError> {
        Ok(match self.table.raw_get::<Value>("weight")? {
            Value::Nil => None,
            Value::Integer(n) => Some(Weight::from_number(n as f64)),
            Value::Number(n) => Some(Weight::from_number(n)),
            Value::String(s) => Some(Weight::parse(&s.to_string_lossy())?),
            _ => return Err(self.error("'weight' must be a number or a name")),
        })
    }

    fn color(&self, name: &str) -> Result<Option<Color>, VisualError> {
        parse_color(self.table.raw_get::<Value>(name)?).map_err(|e| self.error(&e.to_string()))
    }

    fn height(&self) -> Result<f32, VisualError> {
        match self.number("height")? {
            Some(h) => Ok((h as f32).clamp(MIN_HEIGHT, MAX_HEIGHT)),
            None => Err(self.error(&format!(
                "needs height (in pixels, {MIN_HEIGHT}-{MAX_HEIGHT}); its width is the reply \
                 column's"
            ))),
        }
    }

    fn format(&self) -> Result<NumberFormat, VisualError> {
        Ok(NumberFormat {
            prefix: self.string("prefix")?.unwrap_or_default(),
            unit: self.string("unit")?.unwrap_or_default(),
            decimals: self.number("decimals")?.map(|d| d.clamp(0.0, 6.0) as u8),
        })
    }
}

/// A color: a token name, or a `ui.color` / `ui.mix` table.
pub(crate) fn parse_color(value: Value) -> Result<Option<Color>, VisualError> {
    Ok(match value {
        Value::Nil => None,
        Value::String(s) => Some(Color::token(Token::parse(&s.to_string_lossy())?)),
        Value::Table(t) if t.raw_get::<Option<bool>>("__color")?.unwrap_or(false) => {
            let token = Token::parse(&t.raw_get::<String>("token")?)?;
            let mix = match t.raw_get::<Option<String>>("mix")? {
                Some(other) => Some((
                    Token::parse(&other)?,
                    t.raw_get::<Option<f64>>("t")?
                        .unwrap_or(0.5)
                        .clamp(0.0, 1.0) as f32,
                )),
                None => None,
            };
            Some(Color {
                token,
                mix,
                alpha: t
                    .raw_get::<Option<f64>>("alpha")?
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0) as f32,
            })
        }
        other => {
            return Err(VisualError::script(format!(
                "a color is a theme token name or ui.color/ui.mix, not a {}",
                other.type_name()
            )));
        }
    })
}

pub(crate) fn value_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.to_string_lossy(),
        Value::Integer(_) | Value::Number(_) => number_text(value),
        Value::Boolean(b) => b.to_string(),
        Value::Nil => String::new(),
        other => other.type_name().to_owned(),
    }
}

fn number_text(value: &Value) -> String {
    match value {
        Value::Integer(n) => n.to_string(),
        Value::Number(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", *n as i64),
        Value::Number(n) => {
            let text = format!("{n:.4}");
            text.trim_end_matches('0').trim_end_matches('.').to_owned()
        }
        _ => String::new(),
    }
}

fn clip(mut text: String) -> String {
    if text.len() > MAX_TEXT {
        let mut end = MAX_TEXT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

pub(crate) fn short_error(error: &mlua::Error) -> String {
    match error {
        mlua::Error::FromLuaConversionError { from, to, .. } => {
            format!("expected {to}, got {from}")
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_format_with_grouping_and_units() {
        let f = NumberFormat {
            unit: "ms".into(),
            ..Default::default()
        };
        assert_eq!(f.format(1234.0), "1,234 ms");
        assert_eq!(f.format(-12.5), "-12.5 ms");
        let money = NumberFormat {
            prefix: "$".into(),
            decimals: Some(2),
            ..Default::default()
        };
        assert_eq!(money.format(1234567.891), "$1,234,567.89");
        let pct = NumberFormat {
            unit: "%".into(),
            ..Default::default()
        };
        assert_eq!(pct.format(4.256), "4.26%");
    }
}
