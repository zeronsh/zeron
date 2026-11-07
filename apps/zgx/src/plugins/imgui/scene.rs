//! Real 3D inside a plugin window: meshes, camera, and a depth-tested pass.
//!
//! Dear ImGui draws in two dimensions, so a plugin that wants a solid model
//! hands the host a mesh and a camera and the host draws it with wgpu: one
//! pass over the window's own target, depth-tested, lit per face, behind the
//! ImGui draw lists that frame. Nothing here is a polygon sort on the CPU —
//! the GPU owns the z-buffer, so models occlude each other correctly however
//! they are placed.
//!
//! The transform is on this side, in the same spirit as the renderer's
//! vertices: the model, view and projection matrices are multiplied into the
//! vertices before they are uploaded, so the shader needs no uniform buffer
//! and there is nothing to write between frames beyond the vertices
//! themselves.
//!
//! ```text
//! imgui.mesh{ vertices, indices }        a mesh, uploaded once, drawn by handle
//! ui.scene3d{ id, width, height, camera, models }   the rect it fills, drawn
//! ```

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use zgx_script::Value;

use super::style::Resolved;

/// The most vertices one mesh may carry.
pub const MAX_VERTICES: usize = 65_536;
/// The most indices one mesh may carry (three per triangle).
pub const MAX_INDICES: usize = 196_608;
/// The most models one scene may place.
pub const MAX_MODELS: usize = 256;
/// The most scenes one frame may draw: each is a pass of its own.
pub const MAX_SCENES: usize = 4;

/// Floats per vertex a plugin writes: position (3), normal (3), colour (4).
const VERTEX_FLOATS: usize = 10;
/// Bytes of one vertex as the GPU takes it: 3 + 3 floats and four bytes of
/// colour, which is the layout `scene.wgsl` reads.
const GPU_VERTEX_BYTES: usize = 28;

thread_local! {
    /// Mesh ids no two plugins share.
    static NEXT_MESH: Cell<u64> = const { Cell::new(1) };
}

/// One vertex, as a plugin wrote it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub colour: [u8; 4],
}

/// A mesh a plugin registered: triangles, straight from the script.
#[derive(Debug, PartialEq)]
pub struct Mesh {
    pub id: u64,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    /// The mesh `floats` (ten per vertex) and `indices` describe.
    pub fn new(floats: &[Value], indices: &[Value]) -> Result<Self, String> {
        if floats.is_empty() || !floats.len().is_multiple_of(VERTEX_FLOATS) {
            return Err(format!(
                "imgui.mesh needs ten numbers per vertex (x, y, z, nx, ny, nz, r, g, b, a), and it got {}.",
                floats.len()
            ));
        }
        let count = floats.len() / VERTEX_FLOATS;
        if count > MAX_VERTICES {
            return Err(format!(
                "imgui.mesh can carry {MAX_VERTICES} vertices at most, and it was given {count}."
            ));
        }
        if indices.is_empty() || !indices.len().is_multiple_of(3) {
            return Err(
                "imgui.mesh needs three indices per triangle (i0, i1, i2, …) to draw anything."
                    .into(),
            );
        }
        if indices.len() > MAX_INDICES {
            return Err(format!(
                "imgui.mesh can carry {MAX_INDICES} indices at most, and it was given {}.",
                indices.len()
            ));
        }
        let mut vertices = Vec::with_capacity(count);
        for at in 0..count {
            let base = at * VERTEX_FLOATS;
            let number = |offset: usize| -> Result<f32, String> {
                match floats.get(base + offset) {
                    Some(Value::Num(value)) if value.is_finite() => Ok(*value as f32),
                    _ => Err(format!(
                        "imgui.mesh needs ten finite numbers per vertex, and vertex {at} has one that is not."
                    )),
                }
            };
            let colour = |offset: usize| -> Result<u8, String> {
                let value: f64 = match floats.get(base + offset) {
                    Some(Value::Num(value)) if value.is_finite() => *value,
                    _ => {
                        return Err(format!(
                            "imgui.mesh needs ten finite numbers per vertex, and vertex {at} has one that is not."
                        ));
                    }
                };
                // Colours are given 0 to 1, as every other colour here is.
                Ok((value.clamp(0.0, 1.0) * 255.0).round() as u8)
            };
            vertices.push(Vertex {
                position: [number(0)?, number(1)?, number(2)?],
                normal: [number(3)?, number(4)?, number(5)?],
                colour: [colour(6)?, colour(7)?, colour(8)?, colour(9)?],
            });
        }
        let mut corners = Vec::with_capacity(indices.len());
        for (at, index) in indices.iter().enumerate() {
            let Some(Value::Num(index)) = Some(index) else {
                unreachable!()
            };
            if !index.is_finite() || *index < 0.0 || *index as usize >= count {
                return Err(format!(
                    "imgui.mesh needs every index to name one of its {count} vertices, and index {at} is {index}."
                ));
            }
            corners.push(*index as u32);
        }
        let id = NEXT_MESH.with(|next| {
            let id = next.get();
            next.set(id + 1);
            id
        });
        Ok(Self {
            id,
            vertices,
            indices: corners,
        })
    }
}

/// A plugin's meshes, by the handle it was given.
#[derive(Default)]
pub struct PluginMeshes {
    meshes: RefCell<Vec<Rc<Mesh>>>,
}

impl PluginMeshes {
    pub fn new() -> Self {
        Self::default()
    }

    /// `imgui.mesh{ vertices, indices }`.
    pub fn register(&self, arg: &Value) -> Result<Rc<Mesh>, String> {
        let Some(Value::List(floats)) = arg.field("vertices") else {
            return Err(
                "imgui.mesh needs vertices, ten numbers per vertex: x, y, z, nx, ny, nz, r, g, b, a."
                    .into(),
            );
        };
        let Some(Value::List(indices)) = arg.field("indices") else {
            return Err("imgui.mesh needs indices, three per triangle.".into());
        };
        let mesh = Rc::new(Mesh::new(floats, indices)?);
        self.meshes.borrow_mut().push(mesh.clone());
        Ok(mesh)
    }

    /// The mesh `id` names, if this plugin made it.
    pub fn get(&self, id: u64) -> Option<Rc<Mesh>> {
        self.meshes
            .borrow()
            .iter()
            .find(|mesh| mesh.id == id)
            .cloned()
    }

    pub fn holds(&self, id: u64) -> bool {
        self.meshes.borrow().iter().any(|mesh| mesh.id == id)
    }
}

/// Where a scene was asked for, and what it draws: parsed during the frame,
/// built into vertices once the frame's target size is known.
#[derive(Clone, Debug)]
pub struct SceneSpec {
    /// The rect the widget reserved, in points.
    pub rect: [f32; 4],
    pub camera: Camera,
    pub models: Vec<ModelPlacement>,
}

/// An orbit camera: where it looks from, and how wide its lens is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: [f32; 3],
    pub fov: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            yaw: 0.6,
            pitch: -0.35,
            distance: 6.0,
            target: [0.0, 0.0, 0.0],
            fov: 45.0,
            near: 0.05,
            far: 200.0,
        }
    }
}

/// One mesh in a scene: where it sits, how it is turned, what it is tinted.
#[derive(Clone, Debug)]
pub struct ModelPlacement {
    pub mesh: Rc<Mesh>,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
    pub tint: [f32; 4],
}

/// One scene, ready for the GPU: clip-space vertices, indices and the rect
/// of the target it is scissored to.
#[derive(Clone, Debug, Default)]
pub struct PreparedScene {
    /// In device pixels: left, top, width, height.
    pub clip: [u32; 4],
    /// Interleaved position (3 × f32), normal (3 × f32), colour (unorm8 × 4).
    pub vertices: Vec<u8>,
    pub indices: Vec<u32>,
}

impl SceneSpec {
    /// Every scene this frame asked for, built once the frame's target size
    /// is known.
    pub fn build_all(
        specs: &[SceneSpec],
        target_size: (u32, u32),
        scale: f32,
    ) -> Vec<PreparedScene> {
        specs
            .iter()
            .map(|spec| PreparedScene::build(spec, target_size, scale))
            .collect()
    }
}

impl PreparedScene {
    /// The scene `spec` draws at `target_size` device pixels, its rect taken
    /// from the frame's points at `scale`.
    pub fn build(spec: &SceneSpec, target_size: (u32, u32), scale: f32) -> Self {
        let left = (spec.rect[0] * scale).round().max(0.0) as u32;
        let top = (spec.rect[1] * scale).round().max(0.0) as u32;
        let right = (spec.rect[2] * scale)
            .round()
            .max(0.0)
            .min(target_size.0 as f32) as u32;
        let bottom = (spec.rect[3] * scale)
            .round()
            .max(0.0)
            .min(target_size.1 as f32) as u32;
        let clip = [
            left.min(target_size.0),
            top.min(target_size.1),
            right.saturating_sub(left),
            bottom.saturating_sub(top),
        ];
        let aspect = if clip[3] > 0 {
            clip[2] as f32 / clip[3] as f32
        } else {
            1.0
        };
        let view = look_at(spec.camera);
        let projection = perspective(spec.camera, aspect);
        let view_projection = multiply(projection, view);

        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for model in &spec.models {
            let model_matrix = transform(model);
            let normal_matrix = normal_matrix(model);
            let mvp = multiply(view_projection, model_matrix);
            let base = (vertices.len() / GPU_VERTEX_BYTES) as u32;
            for vertex in &model.mesh.vertices {
                let clip = apply(mvp, vertex.position);
                let normal = unit(apply3(normal_matrix, vertex.normal));
                let colour = vertex.colour;
                let tint = model.tint;
                push_vertex(&mut vertices, clip, normal, colour, tint);
            }
            for index in &model.mesh.indices {
                indices.push(base + index);
            }
        }
        Self {
            clip,
            vertices,
            indices,
        }
    }
}

fn push_vertex(
    vertices: &mut Vec<u8>,
    clip: [f32; 3],
    normal: [f32; 3],
    colour: [u8; 4],
    tint: [f32; 4],
) {
    for value in clip {
        vertices.extend_from_slice(&value.to_ne_bytes());
    }
    for value in normal {
        vertices.extend_from_slice(&value.to_ne_bytes());
    }
    let shaded = |value: u8, factor: f32| (f32::from(value) * factor) as u8;
    vertices.extend_from_slice(&[
        shaded(colour[0], tint[0]),
        shaded(colour[1], tint[1]),
        shaded(colour[2], tint[2]),
        shaded(colour[3], tint[3]),
    ]);
}

/// A model's own matrix: scale, then turn about x, y and z, then move.
fn transform(model: &ModelPlacement) -> [[f32; 4]; 4] {
    let scale = matrix_scale(model.scale);
    let rotation = multiply(
        multiply(
            matrix_rotate_z(model.rotation[2]),
            matrix_rotate_y(model.rotation[1]),
        ),
        matrix_rotate_x(model.rotation[0]),
    );
    let turned = multiply(rotation, scale);
    let mut moved = turned;
    moved[3] = [model.position[0], model.position[1], model.position[2], 1.0];
    moved
}

/// The rotation and scale part alone, for normals (this scene's scales are
/// uniform enough that the transpose-inverse is the rotation itself).
fn normal_matrix(model: &ModelPlacement) -> [[f32; 4]; 4] {
    multiply(
        multiply(
            matrix_rotate_z(model.rotation[2]),
            matrix_rotate_y(model.rotation[1]),
        ),
        matrix_rotate_x(model.rotation[0]),
    )
}

/// The orbit camera's view matrix.
fn look_at(camera: Camera) -> [[f32; 4]; 4] {
    let cos_pitch = camera.pitch.cos();
    let eye = [
        camera.target[0] + camera.distance * cos_pitch * camera.yaw.sin(),
        camera.target[1] + camera.distance * camera.pitch.sin(),
        camera.target[2] + camera.distance * cos_pitch * camera.yaw.cos(),
    ];
    let forward = unit([
        camera.target[0] - eye[0],
        camera.target[1] - eye[1],
        camera.target[2] - eye[2],
    ]);
    let right = unit(cross(forward, [0.0, 1.0, 0.0]));
    let up = cross(right, forward);
    [
        [right[0], up[0], -forward[0], 0.0],
        [right[1], up[1], -forward[1], 0.0],
        [right[2], up[2], -forward[2], 0.0],
        [-dot(right, eye), -dot(up, eye), dot(forward, eye), 1.0],
    ]
}

/// A perspective projection, right-handed, looking down -z.
fn perspective(camera: Camera, aspect: f32) -> [[f32; 4]; 4] {
    let fov = camera.fov.clamp(5.0, 150.0).to_radians();
    let focal = 1.0 / (fov / 2.0).tan();
    let near = camera.near.max(0.001);
    let far = camera.far.max(near + 0.001);
    // wgpu clips to z in 0..1 (not OpenGL's -1..1), so the depth row is the
    // zero-to-one one.
    let mut matrix = [[0.0; 4]; 4];
    matrix[0][0] = focal / aspect.max(0.01);
    matrix[1][1] = focal;
    matrix[2][2] = far / (near - far);
    matrix[2][3] = -1.0;
    matrix[3][2] = far * near / (near - far);
    matrix
}

fn matrix_scale(scale: [f32; 3]) -> [[f32; 4]; 4] {
    let mut matrix = identity();
    matrix[0][0] = scale[0];
    matrix[1][1] = scale[1];
    matrix[2][2] = scale[2];
    matrix
}

fn matrix_rotate_x(angle: f32) -> [[f32; 4]; 4] {
    let (sin, cos) = angle.sin_cos();
    let mut matrix = identity();
    matrix[1][1] = cos;
    matrix[1][2] = sin;
    matrix[2][1] = -sin;
    matrix[2][2] = cos;
    matrix
}

fn matrix_rotate_y(angle: f32) -> [[f32; 4]; 4] {
    let (sin, cos) = angle.sin_cos();
    let mut matrix = identity();
    matrix[0][0] = cos;
    matrix[0][2] = -sin;
    matrix[2][0] = sin;
    matrix[2][2] = cos;
    matrix
}

fn matrix_rotate_z(angle: f32) -> [[f32; 4]; 4] {
    let (sin, cos) = angle.sin_cos();
    let mut matrix = identity();
    matrix[0][0] = cos;
    matrix[0][1] = sin;
    matrix[1][0] = -sin;
    matrix[1][1] = cos;
    matrix
}

fn identity() -> [[f32; 4]; 4] {
    let mut matrix = [[0.0; 4]; 4];
    matrix[0][0] = 1.0;
    matrix[1][1] = 1.0;
    matrix[2][2] = 1.0;
    matrix[3][3] = 1.0;
    matrix
}

fn multiply(left: [[f32; 4]; 4], right: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    for column in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for step in 0..4 {
                sum += left[step][row] * right[column][step];
            }
            out[column][row] = sum;
        }
    }
    out
}

fn apply(matrix: [[f32; 4]; 4], point: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0; 3];
    for row in 0..3 {
        out[row] = matrix[0][row] * point[0]
            + matrix[1][row] * point[1]
            + matrix[2][row] * point[2]
            + matrix[3][row];
    }
    let w =
        matrix[0][3] * point[0] + matrix[1][3] * point[1] + matrix[2][3] * point[2] + matrix[3][3];
    if w.abs() > f32::EPSILON {
        [out[0] / w, out[1] / w, out[2] / w]
    } else {
        out
    }
}

fn apply3(matrix: [[f32; 4]; 4], direction: [f32; 3]) -> [f32; 3] {
    [
        matrix[0][0] * direction[0] + matrix[1][0] * direction[1] + matrix[2][0] * direction[2],
        matrix[0][1] * direction[0] + matrix[1][1] * direction[1] + matrix[2][1] * direction[2],
        matrix[0][2] * direction[0] + matrix[1][2] * direction[1] + matrix[2][2] * direction[2],
    ]
}

fn cross(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

fn unit(value: [f32; 3]) -> [f32; 3] {
    let length = dot(value, value).sqrt();
    if length > f32::EPSILON {
        [value[0] / length, value[1] / length, value[2] / length]
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// `ui.scene3d{ … }`: the camera, the models and the background, checked.
pub fn parse_scene(
    arg: &Value,
    resolved: &Resolved,
    meshes: &PluginMeshes,
) -> Result<SceneSpec, String> {
    let camera = parse_camera(arg.field("camera"), resolved)?;
    let Some(Value::List(models)) = arg.field("models") else {
        return Err("ui.scene3d needs models, like { models = { { mesh = handle } } }.".into());
    };
    if models.len() > MAX_MODELS {
        return Err(format!(
            "ui.scene3d can place {MAX_MODELS} models at most, and it was given {}.",
            models.len()
        ));
    }
    let mut placements = Vec::with_capacity(models.len());
    for (at, model) in models.iter().enumerate() {
        if !matches!(model, Value::Map(_)) {
            return Err(format!(
                "ui.scene3d needs every model to be a table, not {}.",
                super::frame::type_name(model)
            ));
        }
        let mesh = model_handle(meshes, model, at)?;
        placements.push(ModelPlacement {
            mesh,
            position: vector_of(model, "position", [0.0; 3])?,
            rotation: vector_of(model, "rotation", [0.0; 3])?,
            scale: scale_of(model)?,
            tint: match model.field("color").or_else(|| model.field("colour")) {
                None => [1.0, 1.0, 1.0, 1.0],
                Some(value) => {
                    let text = match value {
                        Value::Str(text) => text.to_string(),
                        Value::Text(text) => text.to_string(),
                        other => {
                            return Err(format!(
                                "ui.scene3d needs a model's color to be a colour string, not {}.",
                                super::frame::type_name(other)
                            ));
                        }
                    };
                    resolved.colour(&text).ok_or_else(|| {
                        format!(
                            "ui.scene3d can't use colour \"{text}\": colour must be \"#rrggbb\", \"#rrggbbaa\" or a token like \"$accent\"."
                        )
                    })?
                }
            },
        });
    }
    Ok(SceneSpec {
        rect: [0.0; 4],
        camera,
        models: placements,
    })
}

fn model_handle(meshes: &PluginMeshes, model: &Value, at: usize) -> Result<Rc<Mesh>, String> {
    let Some(value) = model.field("mesh") else {
        return Err(format!("ui.scene3d's model {at} needs a mesh handle."));
    };
    let id = match value {
        Value::Num(id) if id.is_finite() && *id >= 0.0 => *id as u64,
        other => {
            return Err(format!(
                "ui.scene3d's model {at} needs mesh to be a handle from imgui.mesh, not {}.",
                super::frame::type_name(other)
            ));
        }
    };
    meshes.get(id).ok_or_else(|| {
        format!(
            "ui.scene3d's model {at} needs a mesh handle this plugin made with imgui.mesh{{ … }}."
        )
    })
}

fn parse_camera(value: Option<&Value>, resolved: &Resolved) -> Result<Camera, String> {
    let mut camera = Camera::default();
    let Some(value) = value else {
        return Ok(camera);
    };
    if !matches!(value, Value::Map(_)) {
        return Err("ui.scene3d needs camera to be a table.".into());
    }
    let _ = resolved;
    camera.yaw = number_of(value, "yaw")?.unwrap_or(camera.yaw);
    camera.pitch = number_of(value, "pitch")?
        .unwrap_or(camera.pitch)
        .clamp(-1.55, 1.55);
    camera.distance = number_of(value, "distance")?
        .unwrap_or(camera.distance)
        .clamp(0.2, 500.0);
    camera.fov = number_of(value, "fov")?
        .unwrap_or(camera.fov)
        .clamp(5.0, 150.0);
    if let Some(target) = value.field("target") {
        camera.target = point_of(target, "target")?;
    }
    Ok(camera)
}

fn number_of(value: &Value, field: &str) -> Result<Option<f32>, String> {
    match value.field(field) {
        None => Ok(None),
        Some(Value::Num(number)) if number.is_finite() => Ok(Some(*number as f32)),
        Some(Value::Num(_)) => Err(format!("ui.scene3d needs {field} to be a finite number.")),
        Some(other) => Err(format!(
            "ui.scene3d needs {field} to be a number, not {}.",
            super::frame::type_name(other)
        )),
    }
}

fn vector_of(value: &Value, field: &str, default: [f32; 3]) -> Result<[f32; 3], String> {
    match value.field(field) {
        None => Ok(default),
        Some(point) => point_of(point, field),
    }
}

fn point_of(value: &Value, field: &str) -> Result<[f32; 3], String> {
    let pair = match value {
        Value::List(items) if items.len() >= 3 => Some((&items[0], &items[1], &items[2])),
        Value::Map(_) => Some((
            value.field("x").unwrap_or(&Value::Nil),
            value.field("y").unwrap_or(&Value::Nil),
            value.field("z").unwrap_or(&Value::Nil),
        )),
        _ => None,
    };
    let read = |value: &Value| match value {
        Value::Num(number) if number.is_finite() => Some(*number as f32),
        _ => None,
    };
    match pair {
        Some((x, y, z)) => match (read(x), read(y), read(z)) {
            (Some(x), Some(y), Some(z)) => Ok([x, y, z]),
            _ => Err(format!(
                "ui.scene3d needs {field} to be three numbers, like {{ 0, 1, 0 }}."
            )),
        },
        None => Err(format!(
            "ui.scene3d needs {field} to be three numbers, like {{ 0, 1, 0 }}."
        )),
    }
}

fn scale_of(model: &Value) -> Result<[f32; 3], String> {
    match model.field("scale") {
        None => Ok([1.0, 1.0, 1.0]),
        Some(Value::Num(scale)) if scale.is_finite() => Ok([*scale as f32; 3]),
        Some(point) => point_of(point, "scale"),
    }
}
