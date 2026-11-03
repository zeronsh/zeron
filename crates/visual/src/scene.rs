//! `ui.scene` 3D: the draw function gets a scene object whose methods add
//! world-space geometry (y up). Zeron frames the camera on the geometry,
//! orbits it on drag, shades faces with one soft light, and projects
//! everything to depth-sorted 2D polygons that the desktop paints like any
//! other vector shape, so a 3D view stays transparent over glass and uses the
//! theme's colors.

use std::sync::Arc;

use mlua::{Function, Table, UserData, UserDataMethods, Value};

use crate::draw::lua_error;
use crate::node::{parse_color, value_text};
use crate::style::{Color, TextSize, Token};

/// Faces a scene may draw (glTF models bring many).
pub const MAX_FACES: usize = 500_000;
const MAX_ITEMS: usize = 20_000;

pub type V3 = [f32; 3];

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Face {
    pub(crate) v: [V3; 4],
    /// 3 or 4 corners.
    pub(crate) n: u8,
    pub(crate) normal: V3,
    /// Per-corner normals of a curved solid (spheres, cylinder sides), so it
    /// shades smoothly instead of faceted.
    pub(crate) normals: Option<[V3; 4]>,
    pub(crate) color: Color,
    /// Closed solids cull faces turned away from the camera; open surfaces
    /// and meshes light both sides.
    pub(crate) two_sided: bool,
    pub(crate) material: Material,
    /// Texture coordinates per corner, for a textured face.
    pub(crate) uvs: Option<[[f32; 2]; 4]>,
    /// Index into the scene's textures.
    pub(crate) texture: Option<u32>,
}

/// An image a scene samples: an asset still encoded (PNG, JPEG, HDR), or
/// pixels a model brought already decoded.
#[derive(Debug, Clone, PartialEq)]
pub enum TextureData {
    Encoded(Arc<[u8]>),
    Rgba {
        width: u32,
        height: u32,
        pixels: Arc<[u8]>,
    },
}

/// One texture a scene uses; `key` names it stably across frames (so a
/// renderer decodes and uploads it once).
#[derive(Debug, Clone, PartialEq)]
pub struct TextureSource {
    pub key: String,
    pub data: TextureData,
}

/// An environment map: an equirectangular image the scene reflects and
/// glass shows when nothing stands behind it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Environment {
    pub texture: u32,
    pub intensity: f32,
    /// Whether the scene stands in front of the map (else the chat shows
    /// behind it, and the map only lights and reflects).
    pub background: bool,
}

/// A camera-facing image (a flare, a glint, a soft glow), drawn as added
/// light.
#[derive(Debug, Clone, PartialEq)]
pub struct Sprite {
    pub at: V3,
    /// Pixels across.
    pub size: f32,
    pub texture: Option<u32>,
    pub color: Color,
    pub glow: f32,
}

/// The degrees `{x, y, z}` (or one number, about y) as a rotation, applied
/// about x, then y, then z.
fn rotation_of(value: Value) -> mlua::Result<[V3; 3]> {
    let [x, y, z] = match value {
        Value::Nil => return Ok([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]),
        Value::Integer(n) => [0.0, n as f32, 0.0],
        Value::Number(n) => [0.0, n as f32, 0.0],
        other => v3(other, "rotate")?,
    }
    .map(f32::to_radians);
    let (sx, cx, sy, cy, sz, cz) = (x.sin(), x.cos(), y.sin(), y.cos(), z.sin(), z.cos());
    // Rows of Rz * Ry * Rx.
    Ok([
        [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
        [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
        [-sy, cy * sx, cy * cx],
    ])
}

fn rotate(m: &[V3; 3], v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

/// How a surface takes light. `glass` lets light through (refracting by
/// `ior`, splitting into colors by `dispersion`); `glow` emits light of the
/// surface's own color, which blooms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Material {
    /// 0 is a mirror finish, 1 fully matte.
    pub roughness: f32,
    /// 0 is a dielectric (plastic, wood), 1 a metal.
    pub metalness: f32,
    pub glow: f32,
    /// 0 opaque to 1 clear glass.
    pub glass: f32,
    pub ior: f32,
    pub dispersion: f32,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            roughness: 0.55,
            metalness: 0.0,
            glow: 0.0,
            glass: 0.0,
            ior: 1.5,
            dispersion: 0.0,
        }
    }
}

/// A face as the GPU renderer reads it.
pub struct FaceView<'a> {
    pub corners: &'a [V3],
    pub normal: V3,
    /// Per-corner normals of a curved solid.
    pub normals: Option<&'a [V3]>,
    pub color: Color,
    pub two_sided: bool,
    pub material: Material,
    pub uvs: Option<&'a [[f32; 2]]>,
    pub texture: Option<u32>,
}

/// The framed orbit camera, for a renderer that does its own projection.
#[derive(Debug, Clone, Copy)]
pub struct CameraFrame {
    pub eye: V3,
    pub right: V3,
    pub up: V3,
    /// Into the screen.
    pub forward: V3,
    /// Pixels per unit at unit depth, for the canvas the frame was made for.
    pub focal: f32,
    pub near: f32,
    /// The geometry's center and bounding radius.
    pub center: V3,
    pub radius: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Line3 {
    pub(crate) a: V3,
    pub(crate) b: V3,
    pub(crate) color: Color,
    pub(crate) width: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Point3 {
    pub(crate) at: V3,
    pub(crate) radius: f32,
    pub(crate) color: Color,
    pub(crate) hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Label3 {
    pub(crate) at: V3,
    pub(crate) text: String,
    pub(crate) color: Color,
    pub(crate) size: TextSize,
}

/// The geometry one draw call produced.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Scene {
    pub(crate) faces: Vec<Face>,
    pub(crate) lines: Vec<Line3>,
    pub(crate) points: Vec<Point3>,
    pub(crate) labels: Vec<Label3>,
    pub time: f64,
    /// The material the primitive being added gives its faces.
    pub(crate) material: Material,
    /// The texture the primitive being added gives its faces.
    pub(crate) texture: Option<u32>,
    pub(crate) textures: Vec<TextureSource>,
    pub(crate) environment: Option<Environment>,
    pub(crate) sprites: Vec<Sprite>,
    pub(crate) assets: crate::assets::Assets,
}

/// The camera a scene is seen through: framed on its geometry and orbited.
pub(crate) struct View {
    pub(crate) eye: V3,
    right: V3,
    up: V3,
    forward: V3,
    focal: f32,
    pub(crate) near: f32,
    cx: f32,
    cy: f32,
    pub(crate) center: V3,
    /// The geometry's bounding radius.
    pub(crate) radius: f32,
    /// The soft key light the painter shades with, from the upper left.
    pub(crate) light: V3,
}

impl View {
    /// World to view space: x right, y up, z into the screen.
    pub(crate) fn view(&self, p: V3) -> V3 {
        let d = sub(p, self.eye);
        [dot(d, self.right), dot(d, self.up), dot(d, self.forward)]
    }

    /// View space to canvas pixels.
    pub(crate) fn screen(&self, v: V3) -> [f32; 2] {
        [
            self.cx + self.focal * v[0] / v[2],
            self.cy - self.focal * v[1] / v[2],
        ]
    }
}

/// The orbit camera: angles in radians around the geometry's center.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
}

/// One painted primitive, in canvas pixels.
#[derive(Debug, Clone, PartialEq)]
pub enum Prim {
    Face {
        points: Vec<[f32; 2]>,
        color: Color,
        /// Lighting, 0 (unlit) to 1 (facing the light).
        shade: f32,
    },
    Line {
        a: [f32; 2],
        b: [f32; 2],
        color: Color,
        width: f32,
    },
    Dot {
        at: [f32; 2],
        radius: f32,
        color: Color,
    },
    Label {
        at: [f32; 2],
        text: String,
        color: Color,
        size: TextSize,
    },
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Projection {
    /// Back to front; labels last.
    pub prims: Vec<Prim>,
    /// Hover hints: screen position, radius, text.
    pub hints: Vec<([f32; 2], f32, String)>,
}

pub(crate) fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(crate) fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub(crate) fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub(crate) fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub(crate) fn normalize(a: V3) -> V3 {
    let len = dot(a, a).sqrt();
    if len < 1e-9 {
        [0.0, 1.0, 0.0]
    } else {
        scale(a, 1.0 / len)
    }
}

impl Scene {
    pub fn new(time: f64) -> Self {
        Self {
            time,
            ..Default::default()
        }
    }

    /// A scene whose draw may use `assets` (textures, models).
    pub fn with_assets(time: f64, assets: crate::assets::Assets) -> Self {
        Self {
            time,
            assets,
            ..Default::default()
        }
    }

    /// The texture keyed `key`, registered once; its index.
    fn texture_index(&mut self, key: String, data: impl FnOnce() -> TextureData) -> u32 {
        if let Some(i) = self.textures.iter().position(|t| t.key == key) {
            return i as u32;
        }
        self.textures.push(TextureSource { key, data: data() });
        (self.textures.len() - 1) as u32
    }

    /// The asset `name` as a texture.
    fn asset_texture(&mut self, name: &str) -> mlua::Result<u32> {
        let bytes = self.assets.require(name).map_err(lua_error)?;
        Ok(self.texture_index(format!("asset:{name}"), || TextureData::Encoded(bytes)))
    }

    /// The asset `name` parsed as a glTF model, once per visual.
    fn model(&self, name: &str) -> mlua::Result<Arc<crate::model::Model>> {
        let bytes = self.assets.require(name).map_err(lua_error)?;
        let mut models = self
            .assets
            .models
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(model) = models.get(name) {
            return Ok(model.clone());
        }
        let model = crate::model::parse_shared(&bytes)
            .map_err(|e| lua_error(format!("model '{name}': {e}")))?;
        models.insert(name.to_owned(), model.clone());
        Ok(model)
    }

    /// Give the face just added its texture coordinates.
    fn uv_last(&mut self, uvs: &[[f32; 2]]) {
        if let Some(face) = self.faces.last_mut() {
            let mut out = [[0.0; 2]; 4];
            out[..uvs.len().min(4)].copy_from_slice(&uvs[..uvs.len().min(4)]);
            face.uvs = Some(out);
        }
    }

    pub fn textures(&self) -> &[TextureSource] {
        &self.textures
    }

    pub fn environment(&self) -> Option<Environment> {
        self.environment
    }

    pub fn sprites(&self) -> &[Sprite] {
        &self.sprites
    }

    pub(crate) fn face(
        &mut self,
        corners: &[V3],
        color: Color,
        outward: Option<V3>,
        two_sided: bool,
    ) -> mlua::Result<()> {
        if self.faces.len() >= MAX_FACES {
            return Err(lua_error(format!(
                "a scene draws at most {MAX_FACES} faces"
            )));
        }
        if corners.iter().flatten().any(|c| !c.is_finite()) {
            return Err(lua_error("a scene vertex is NaN or infinite"));
        }
        let mut normal = normalize(cross(
            sub(corners[1], corners[0]),
            sub(corners[corners.len() - 1], corners[0]),
        ));
        if corners.len() == 4 {
            // A non-planar quad: average both diagonals' normals.
            normal = normalize(cross(
                sub(corners[2], corners[0]),
                sub(corners[3], corners[1]),
            ));
        }
        if let Some(outward) = outward
            && dot(normal, outward) < 0.0
        {
            normal = scale(normal, -1.0);
        }
        let mut v = [[0.0; 3]; 4];
        v[..corners.len()].copy_from_slice(corners);
        self.faces.push(Face {
            v,
            n: corners.len() as u8,
            normal,
            normals: None,
            color,
            two_sided,
            material: self.material,
            uvs: None,
            texture: self.texture,
        });
        Ok(())
    }

    /// A face of a curved solid, shaded from the surface's own normals.
    pub(crate) fn smooth_face(
        &mut self,
        corners: &[V3],
        normals: &[V3],
        color: Color,
    ) -> mlua::Result<()> {
        let outward = normals.iter().fold([0.0; 3], |acc, n| add(acc, *n));
        self.face(corners, color, Some(outward), false)?;
        let mut n = [[0.0; 3]; 4];
        for (slot, normal) in n.iter_mut().zip(normals) {
            *slot = normalize(*normal);
        }
        if let Some(face) = self.faces.last_mut() {
            face.normals = Some(n);
        }
        Ok(())
    }

    fn polygon(
        &mut self,
        corners: &[V3],
        color: Color,
        outward: Option<V3>,
        two_sided: bool,
    ) -> mlua::Result<()> {
        match corners.len() {
            0..=2 => Ok(()),
            3 | 4 => self.face(corners, color, outward, two_sided),
            _ => {
                for i in 1..corners.len() - 1 {
                    self.face(
                        &[corners[0], corners[i], corners[i + 1]],
                        color,
                        outward,
                        two_sided,
                    )?;
                }
                Ok(())
            }
        }
    }

    fn items(&self) -> usize {
        self.lines.len() + self.points.len() + self.labels.len() + self.sprites.len()
    }

    fn check_items(&self) -> mlua::Result<()> {
        if self.items() >= MAX_ITEMS {
            return Err(lua_error(format!(
                "a scene draws at most {MAX_ITEMS} lines, points, and labels"
            )));
        }
        Ok(())
    }

    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    fn bounds(&self) -> Option<(V3, V3)> {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        let mut grow = |p: V3| {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        };
        for face in &self.faces {
            for v in &face.v[..face.n as usize] {
                grow(*v);
            }
        }
        for line in &self.lines {
            grow(line.a);
            grow(line.b);
        }
        for point in &self.points {
            grow(point.at);
        }
        for label in &self.labels {
            grow(label.at);
        }
        lo[0].is_finite().then_some((lo, hi))
    }

    /// The camera framed on this geometry and orbited to `camera`, for a
    /// canvas `width` by `height`.
    pub(crate) fn view_for(&self, camera: Camera, width: f32, height: f32) -> Option<View> {
        let (lo, hi) = self.bounds()?;
        let target = scale(add(lo, hi), 0.5);
        let half = scale(sub(hi, lo), 0.5);
        let radius = dot(half, half).sqrt().max(1e-3);
        let pitch = camera.pitch.clamp(-1.52, 1.52);
        // Fit the geometry's extent as the turntable sees it: the footprint's
        // radius sideways, and its height plus the tilted footprint
        // vertically. Neither depends on yaw, so a spinning scene keeps its
        // size.
        let footprint = half[0].hypot(half[2]).max(1e-3);
        let tall = (half[1] * pitch.cos().abs() + footprint * pitch.sin().abs()).max(1e-3);
        let distance = radius * 4.0;
        let eye = add(
            target,
            scale(
                [
                    pitch.cos() * camera.yaw.sin(),
                    pitch.sin(),
                    pitch.cos() * camera.yaw.cos(),
                ],
                distance,
            ),
        );
        let forward = normalize(sub(target, eye));
        let right = normalize(cross(forward, [0.0, 1.0, 0.0]));
        let up = cross(right, forward);
        let focal = 0.86
            * (width / 2.0 / footprint).min(height / 2.0 / tall)
            * distance
            * camera.zoom.max(0.05);
        // A soft key light from the upper left, in front of the geometry.
        let light = normalize(add(
            add(scale(right, -0.45), scale(up, 0.8)),
            scale(forward, -0.55),
        ));
        Some(View {
            eye,
            right,
            up,
            forward,
            focal,
            near: distance * 0.02,
            cx: width / 2.0,
            cy: height / 2.0,
            center: target,
            radius,
            light,
        })
    }

    /// The camera framed on this geometry, for the GPU renderer.
    pub fn camera_frame(&self, camera: Camera, width: f32, height: f32) -> Option<CameraFrame> {
        let view = self.view_for(camera, width, height)?;
        Some(CameraFrame {
            eye: view.eye,
            right: view.right,
            up: view.up,
            forward: view.forward,
            focal: view.focal,
            near: view.near,
            center: view.center,
            radius: view.radius,
        })
    }

    pub fn face_views(&self) -> impl Iterator<Item = FaceView<'_>> {
        self.faces.iter().map(|f| FaceView {
            corners: &f.v[..f.n as usize],
            normal: f.normal,
            normals: f.normals.as_ref().map(|n| &n[..f.n as usize]),
            color: f.color,
            two_sided: f.two_sided,
            material: f.material,
            uvs: f.uvs.as_ref().map(|uv| &uv[..f.n as usize]),
            texture: f.texture,
        })
    }

    /// Lines: ends, color, width in pixels.
    pub fn line_views(&self) -> impl Iterator<Item = (V3, V3, Color, f32)> + '_ {
        self.lines.iter().map(|l| (l.a, l.b, l.color, l.width))
    }

    /// Points: center, radius in pixels, color.
    pub fn point_views(&self) -> impl Iterator<Item = (V3, f32, Color)> + '_ {
        self.points.iter().map(|p| (p.at, p.radius, p.color))
    }

    /// Frame the geometry, orbit to `camera`, light, cull, and depth-sort.
    pub fn project(&self, camera: Camera, width: f32, height: f32) -> Projection {
        let Some(setup) = self.view_for(camera, width, height) else {
            return Projection::default();
        };
        let (eye, near, radius, light) = (setup.eye, setup.near, setup.radius, setup.light);
        let view = |p: V3| setup.view(p);
        let screen = |v: V3| setup.screen(v);

        let mut sorted: Vec<(f32, Prim)> = Vec::with_capacity(self.faces.len() + self.items());
        for face in &self.faces {
            let corners = &face.v[..face.n as usize];
            let viewed: Vec<V3> = corners.iter().map(|v| view(*v)).collect();
            if viewed.iter().any(|v| v[2] < near) {
                continue;
            }
            let centroid = scale(
                corners.iter().fold([0.0; 3], |acc, v| add(acc, *v)),
                1.0 / corners.len() as f32,
            );
            let toward_eye = dot(face.normal, sub(eye, centroid));
            if !face.two_sided && toward_eye < 0.0 {
                continue;
            }
            let mut normal = face.normal;
            if toward_eye < 0.0 {
                normal = scale(normal, -1.0);
            }
            let lambert = dot(normal, light).max(0.0);
            let depth = viewed.iter().map(|v| v[2]).sum::<f32>() / viewed.len() as f32;
            sorted.push((
                depth,
                Prim::Face {
                    points: viewed.iter().map(|v| screen(*v)).collect(),
                    color: face.color,
                    shade: 0.35 + 0.65 * lambert,
                },
            ));
        }
        for line in &self.lines {
            let (a, b) = (view(line.a), view(line.b));
            if a[2] < near || b[2] < near {
                continue;
            }
            sorted.push((
                (a[2] + b[2]) / 2.0 - radius * 1e-3,
                Prim::Line {
                    a: screen(a),
                    b: screen(b),
                    color: line.color,
                    width: line.width,
                },
            ));
        }
        let mut hints = Vec::new();
        for point in &self.points {
            let v = view(point.at);
            if v[2] < near {
                continue;
            }
            let at = screen(v);
            if let Some(hint) = &point.hint {
                hints.push((at, point.radius.max(4.0), hint.clone()));
            }
            sorted.push((
                v[2] - radius * 2e-3,
                Prim::Dot {
                    at,
                    radius: point.radius,
                    color: point.color,
                },
            ));
        }
        sorted.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut prims: Vec<Prim> = sorted.into_iter().map(|(_, p)| p).collect();
        for label in &self.labels {
            let v = view(label.at);
            if v[2] < near {
                continue;
            }
            prims.push(Prim::Label {
                at: screen(v),
                text: label.text.clone(),
                color: label.color,
                size: label.size,
            });
        }
        Projection { prims, hints }
    }
}

fn v3(value: Value, what: &str) -> mlua::Result<V3> {
    match value {
        Value::Table(t) => {
            let get = |i: usize, key: &str| -> mlua::Result<f32> {
                match t.raw_get::<Option<f64>>(i)? {
                    Some(n) => Ok(n as f32),
                    None => Ok(t.raw_get::<Option<f64>>(key)?.unwrap_or(0.0) as f32),
                }
            };
            Ok([get(1, "x")?, get(2, "y")?, get(3, "z")?])
        }
        Value::Vector(v) => Ok([v.x(), v.y(), v.z()]),
        _ => Err(lua_error(format!("{what} must be a {{x, y, z}} list"))),
    }
}

/// A primitive's material options: roughness, metal, glow, glass (true or
/// 0-1), ior, dispersion.
fn material_of(options: &Table) -> mlua::Result<Material> {
    material_over(Material::default(), options)
}

/// `base` with whichever material options `options` sets.
fn material_over(mut m: Material, options: &Table) -> mlua::Result<Material> {
    let number = |key: &str| options.raw_get::<Option<f32>>(key);
    if let Some(v) = number("roughness")? {
        m.roughness = v.clamp(0.02, 1.0);
    }
    if let Some(v) = number("metal")? {
        m.metalness = v.clamp(0.0, 1.0);
    }
    if let Some(v) = number("glow")? {
        m.glow = v.clamp(0.0, 8.0);
    }
    match options.raw_get::<Value>("glass")? {
        Value::Boolean(true) => {
            m.glass = 1.0;
            m.roughness = m.roughness.min(0.08);
        }
        Value::Integer(n) => m.glass = (n as f32).clamp(0.0, 1.0),
        Value::Number(n) => m.glass = (n as f32).clamp(0.0, 1.0),
        _ => {}
    }
    if let Some(v) = number("ior")? {
        m.ior = v.clamp(1.0, 3.0);
    }
    if let Some(v) = number("dispersion")? {
        m.dispersion = v.clamp(0.0, 1.0);
    }
    Ok(m)
}

/// A primitive's `texture = "asset name"`, registered with the scene.
fn texture_of(scene: &mut Scene, options: &Table) -> mlua::Result<Option<u32>> {
    match options.raw_get::<Option<String>>("texture")? {
        Some(name) => scene.asset_texture(&name).map(Some),
        None => Ok(None),
    }
}

fn color_of(options: &Table, default: Token) -> mlua::Result<Color> {
    Ok(scene_color(options.raw_get::<Value>("color")?)?.unwrap_or(Color::token(default)))
}

/// A scene color: a theme token like everywhere else, or, for physical
/// materials, `"#rrggbb"`.
pub(crate) fn scene_color(value: Value) -> mlua::Result<Option<Color>> {
    if let Value::String(s) = &value {
        let text = s.to_string_lossy();
        if let Some(hex) = text.strip_prefix('#') {
            let rgb = match hex.len() {
                6 => u32::from_str_radix(hex, 16).ok(),
                3 => u32::from_str_radix(hex, 16).ok().map(|v| {
                    let (r, g, b) = ((v >> 8) & 0xf, (v >> 4) & 0xf, v & 0xf);
                    ((r * 17) << 16) | ((g * 17) << 8) | (b * 17)
                }),
                _ => None,
            }
            .ok_or_else(|| lua_error(format!("'{text}' is not a #rrggbb color")))?;
            return Ok(Some(Color::token(Token::Rgb(rgb))));
        }
    }
    parse_color(value).map_err(|e| lua_error(e.to_string()))
}

/// A ramp through `colors` (a list of tokens) at `t` in 0..1.
fn ramp(colors: &[Color], t: f32) -> Color {
    match colors.len() {
        0 => Color::token(Token::Accent),
        1 => colors[0],
        n => {
            let scaled = t.clamp(0.0, 1.0) * (n - 1) as f32;
            let i = (scaled.floor() as usize).min(n - 2);
            Color {
                token: colors[i].token,
                mix: Some((colors[i + 1].token, scaled - i as f32)),
                alpha: colors[i].alpha,
            }
        }
    }
}

fn colors_of(options: &Table) -> mlua::Result<Vec<Color>> {
    let Some(list) = options.raw_get::<Option<Table>>("colors")? else {
        return Ok(vec![color_of(options, Token::Accent)?]);
    };
    let mut out = Vec::new();
    for value in list.sequence_values::<Value>() {
        if let Some(color) = parse_color(value?).map_err(|e| lua_error(e.to_string()))? {
            out.push(color);
        }
    }
    Ok(out)
}

impl UserData for Scene {
    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("time", |_, s| Ok(s.time));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("box", |_, s, o: Table| {
            s.material = material_of(&o)?;
            s.texture = texture_of(s, &o)?;
            let at = v3(o.raw_get("at")?, "box at")?;
            let size = match o.raw_get::<Value>("size")? {
                Value::Nil => [1.0; 3],
                Value::Integer(n) => [n as f32; 3],
                Value::Number(n) => [n as f32; 3],
                other => v3(other, "box size")?,
            };
            let color = color_of(&o, Token::Accent)?;
            let h = scale(size, 0.5);
            let c = |sx: f32, sy: f32, sz: f32| {
                [at[0] + sx * h[0], at[1] + sy * h[1], at[2] + sz * h[2]]
            };
            let quads = [
                (
                    [
                        c(-1., -1., 1.),
                        c(1., -1., 1.),
                        c(1., 1., 1.),
                        c(-1., 1., 1.),
                    ],
                    [0., 0., 1.],
                ),
                (
                    [
                        c(1., -1., -1.),
                        c(-1., -1., -1.),
                        c(-1., 1., -1.),
                        c(1., 1., -1.),
                    ],
                    [0., 0., -1.],
                ),
                (
                    [
                        c(1., -1., 1.),
                        c(1., -1., -1.),
                        c(1., 1., -1.),
                        c(1., 1., 1.),
                    ],
                    [1., 0., 0.],
                ),
                (
                    [
                        c(-1., -1., -1.),
                        c(-1., -1., 1.),
                        c(-1., 1., 1.),
                        c(-1., 1., -1.),
                    ],
                    [-1., 0., 0.],
                ),
                (
                    [
                        c(-1., 1., 1.),
                        c(1., 1., 1.),
                        c(1., 1., -1.),
                        c(-1., 1., -1.),
                    ],
                    [0., 1., 0.],
                ),
                (
                    [
                        c(-1., -1., -1.),
                        c(1., -1., -1.),
                        c(1., -1., 1.),
                        c(-1., -1., 1.),
                    ],
                    [0., -1., 0.],
                ),
            ];
            for (quad, outward) in quads {
                s.face(&quad, color, Some(outward), false)?;
                s.uv_last(&[[0., 1.], [1., 1.], [1., 0.], [0., 0.]]);
            }
            Ok(())
        });
        methods.add_method_mut("sphere", |_, s, o: Table| {
            s.material = material_of(&o)?;
            s.texture = texture_of(s, &o)?;
            let at = v3(o.raw_get("at")?, "sphere at")?;
            let r = o.raw_get::<Option<f32>>("radius")?.unwrap_or(0.5).abs();
            let color = color_of(&o, Token::Accent)?;
            let detail = o
                .raw_get::<Option<u32>>("detail")?
                .unwrap_or(18)
                .clamp(6, 48);
            let (lon, lat) = (detail * 2, detail);
            let p = |i: u32, j: u32| {
                let theta = std::f32::consts::TAU * i as f32 / lon as f32;
                let phi = std::f32::consts::PI * j as f32 / lat as f32;
                [
                    at[0] + r * phi.sin() * theta.cos(),
                    at[1] + r * phi.cos(),
                    at[2] + r * phi.sin() * theta.sin(),
                ]
            };
            for j in 0..lat {
                for i in 0..lon {
                    let quad = [p(i, j), p(i + 1, j), p(i + 1, j + 1), p(i, j + 1)];
                    let corners: Vec<V3> = if j == 0 {
                        vec![quad[0], quad[2], quad[3]]
                    } else if j == lat - 1 {
                        vec![quad[0], quad[1], quad[2]]
                    } else {
                        quad.to_vec()
                    };
                    let normals: Vec<V3> = corners.iter().map(|c| sub(*c, at)).collect();
                    s.smooth_face(&corners, &normals, color)?;
                    let uv = |i: u32, j: u32| [i as f32 / lon as f32, j as f32 / lat as f32];
                    let uvq = [uv(i, j), uv(i + 1, j), uv(i + 1, j + 1), uv(i, j + 1)];
                    if j == 0 {
                        s.uv_last(&[uvq[0], uvq[2], uvq[3]]);
                    } else if j == lat - 1 {
                        s.uv_last(&[uvq[0], uvq[1], uvq[2]]);
                    } else {
                        s.uv_last(&uvq);
                    }
                }
            }
            Ok(())
        });
        methods.add_method_mut("cylinder", |_, s, o: Table| {
            s.material = material_of(&o)?;
            s.texture = texture_of(s, &o)?;
            let at = v3(o.raw_get("at")?, "cylinder at")?;
            let r = o.raw_get::<Option<f32>>("radius")?.unwrap_or(0.5).abs();
            let h = o.raw_get::<Option<f32>>("height")?.unwrap_or(1.0);
            let color = color_of(&o, Token::Accent)?;
            let n = o
                .raw_get::<Option<u32>>("detail")?
                .unwrap_or(24)
                .clamp(6, 64);
            let ring = |i: u32, y: f32| {
                let a = std::f32::consts::TAU * i as f32 / n as f32;
                [at[0] + r * a.cos(), at[1] + y, at[2] + r * a.sin()]
            };
            for i in 0..n {
                let quad = [ring(i, 0.0), ring(i + 1, 0.0), ring(i + 1, h), ring(i, h)];
                let normals: Vec<V3> = quad
                    .iter()
                    .map(|c| [c[0] - at[0], 0.0, c[2] - at[2]])
                    .collect();
                s.smooth_face(&quad, &normals, color)?;
                let (u0, u1) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
                s.uv_last(&[[u0, 1.0], [u1, 1.0], [u1, 0.0], [u0, 0.0]]);
            }
            s.texture = None;
            let top: Vec<V3> = (0..n).map(|i| ring(i, h)).collect();
            let bottom: Vec<V3> = (0..n).map(|i| ring(i, 0.0)).collect();
            s.polygon(&top, color, Some([0.0, h.signum(), 0.0]), false)?;
            s.polygon(&bottom, color, Some([0.0, -h.signum(), 0.0]), false)
        });
        // A height field: y = f(x, z) over a grid, or `values` rows.
        methods.add_method_mut("surface", |_, s, o: Table| {
            s.material = material_of(&o)?;
            s.texture = texture_of(s, &o)?;
            let range = |key: &str| -> mlua::Result<(f32, f32)> {
                match o.raw_get::<Option<Table>>(key)? {
                    Some(t) => Ok((t.raw_get::<f32>(1)?, t.raw_get::<f32>(2)?)),
                    None => Ok((-1.0, 1.0)),
                }
            };
            let (x0, x1) = range("x")?;
            let (z0, z1) = range("z")?;
            let colors = colors_of(&o)?;
            let wire = o.raw_get::<Option<bool>>("wire")?.unwrap_or(false);
            let mut grid: Vec<Vec<f32>> = Vec::new();
            if let Some(values) = o.raw_get::<Option<Table>>("values")? {
                for row in values.sequence_values::<Table>() {
                    grid.push(row?.sequence_values::<f32>().collect::<mlua::Result<_>>()?);
                }
            } else {
                let f: Function = o
                    .raw_get::<Option<Function>>("f")?
                    .ok_or_else(|| lua_error("scene:surface needs f = function(x, z) or values"))?;
                let n = o
                    .raw_get::<Option<u32>>("detail")?
                    .unwrap_or(32)
                    .clamp(2, 120) as usize;
                for j in 0..=n {
                    let z = z0 + (z1 - z0) * j as f32 / n as f32;
                    let mut row = Vec::with_capacity(n + 1);
                    for i in 0..=n {
                        let x = x0 + (x1 - x0) * i as f32 / n as f32;
                        row.push(f.call::<f32>((x, z))?);
                    }
                    grid.push(row);
                }
            }
            let rows = grid.len();
            let cols = grid.iter().map(Vec::len).min().unwrap_or(0);
            if rows < 2 || cols < 2 {
                return Err(lua_error("scene:surface needs at least a 2x2 grid"));
            }
            let (lo, hi) = grid
                .iter()
                .flatten()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                    (lo.min(*v), hi.max(*v))
                });
            let span = (hi - lo).max(1e-6);
            let point = |i: usize, j: usize| {
                [
                    x0 + (x1 - x0) * i as f32 / (cols - 1) as f32,
                    grid[j][i],
                    z0 + (z1 - z0) * j as f32 / (rows - 1) as f32,
                ]
            };
            for j in 0..rows - 1 {
                for i in 0..cols - 1 {
                    let quad = [
                        point(i, j),
                        point(i + 1, j),
                        point(i + 1, j + 1),
                        point(i, j + 1),
                    ];
                    let mean = quad.iter().map(|v| v[1]).sum::<f32>() / 4.0;
                    let color = ramp(&colors, (mean - lo) / span);
                    if wire {
                        s.check_items()?;
                        for (a, b) in [(quad[0], quad[1]), (quad[0], quad[3])] {
                            s.lines.push(Line3 {
                                a,
                                b,
                                color,
                                width: 1.0,
                            });
                        }
                    } else {
                        s.face(&quad, color, Some([0.0, 1.0, 0.0]), true)?;
                        let (u0, u1) = (
                            i as f32 / (cols - 1) as f32,
                            (i + 1) as f32 / (cols - 1) as f32,
                        );
                        let (v0, v1) = (
                            j as f32 / (rows - 1) as f32,
                            (j + 1) as f32 / (rows - 1) as f32,
                        );
                        s.uv_last(&[[u0, v0], [u1, v0], [u1, v1], [u0, v1]]);
                    }
                }
            }
            Ok(())
        });
        methods.add_method_mut("mesh", |_, s, o: Table| {
            s.material = material_of(&o)?;
            s.texture = texture_of(s, &o)?;
            let color = color_of(&o, Token::Accent)?;
            let vertices: Vec<V3> = o
                .raw_get::<Table>("vertices")?
                .sequence_values::<Value>()
                .map(|v| v.and_then(|v| v3(v, "mesh vertex")))
                .collect::<mlua::Result<_>>()?;
            let uvs: Option<Vec<[f32; 2]>> = match o.raw_get::<Option<Table>>("uvs")? {
                Some(list) => Some(
                    list.sequence_values::<Table>()
                        .map(|uv| {
                            let uv = uv?;
                            Ok([uv.raw_get::<f32>(1)?, uv.raw_get::<f32>(2)?])
                        })
                        .collect::<mlua::Result<_>>()?,
                ),
                None => None,
            };
            for face in o.raw_get::<Table>("faces")?.sequence_values::<Table>() {
                let indices: Vec<usize> = face?
                    .sequence_values::<usize>()
                    .collect::<mlua::Result<_>>()?;
                let corners: Vec<V3> = indices
                    .iter()
                    .map(|&i| {
                        vertices.get(i.wrapping_sub(1)).copied().ok_or_else(|| {
                            lua_error(format!("mesh face index {i} is out of range"))
                        })
                    })
                    .collect::<mlua::Result<_>>()?;
                match &uvs {
                    Some(uvs) if corners.len() >= 3 => {
                        let uv = |i: usize| {
                            uvs.get(indices[i].wrapping_sub(1))
                                .copied()
                                .unwrap_or([0.0; 2])
                        };
                        // Fan it, so every triangle keeps its own coordinates.
                        for k in 1..corners.len() - 1 {
                            s.face(&[corners[0], corners[k], corners[k + 1]], color, None, true)?;
                            s.uv_last(&[uv(0), uv(k), uv(k + 1)]);
                        }
                    }
                    _ => s.polygon(&corners, color, None, true)?,
                }
            }
            Ok(())
        });
        methods.add_method_mut("line", |_, s, (a, b, o): (Value, Value, Option<Table>)| {
            s.check_items()?;
            let (a, b, o) = match (&a, &b) {
                (Value::Table(t), Value::Nil)
                    if t.raw_get::<Value>("from").is_ok_and(|v| !v.is_nil()) =>
                {
                    (t.raw_get("from")?, t.raw_get("to")?, Some(t.clone()))
                }
                _ => (a, b, o),
            };
            let (color, width) = line_style(o)?;
            s.lines.push(Line3 {
                a: v3(a, "line start")?,
                b: v3(b, "line end")?,
                color,
                width,
            });
            Ok(())
        });
        methods.add_method_mut("path", |_, s, (points, o): (Table, Option<Table>)| {
            let (color, width) = line_style(o)?;
            let pts: Vec<V3> = points
                .sequence_values::<Value>()
                .map(|v| v.and_then(|v| v3(v, "path point")))
                .collect::<mlua::Result<_>>()?;
            for pair in pts.windows(2) {
                s.check_items()?;
                s.lines.push(Line3 {
                    a: pair[0],
                    b: pair[1],
                    color,
                    width,
                });
            }
            Ok(())
        });
        methods.add_method_mut("point", |_, s, (at, o): (Value, Option<Table>)| {
            s.check_items()?;
            let (at, o) = options_form(at, o)?;
            let (color, radius, hint) = match o {
                Some(o) => (
                    color_of(&o, Token::Accent)?,
                    o.raw_get::<Option<f32>>("radius")?
                        .unwrap_or(3.0)
                        .clamp(0.5, 24.0),
                    o.raw_get::<Value>("hint")
                        .map(|v| (!v.is_nil()).then(|| value_text(&v)))?,
                ),
                None => (Color::token(Token::Accent), 3.0, None),
            };
            s.points.push(Point3 {
                at: v3(at, "point")?,
                radius,
                color,
                hint,
            });
            Ok(())
        });
        methods.add_method_mut(
            "label",
            |_, s, (at, text, o): (Value, Value, Option<Table>)| {
                s.check_items()?;
                let (at, text, o) = match (&at, &text) {
                    (Value::Table(t), Value::Nil)
                        if t.raw_get::<Value>("at").is_ok_and(|v| !v.is_nil()) =>
                    {
                        (t.raw_get("at")?, t.raw_get("text")?, Some(t.clone()))
                    }
                    _ => (at, text, o),
                };
                let (color, size) = match o {
                    Some(o) => (
                        color_of(&o, Token::Muted)?,
                        match o.raw_get::<Option<String>>("size")? {
                            Some(size) => {
                                TextSize::parse(&size).map_err(|e| lua_error(e.to_string()))?
                            }
                            None => TextSize::Xs,
                        },
                    ),
                    None => (Color::token(Token::Muted), TextSize::Xs),
                };
                s.labels.push(Label3 {
                    at: v3(at, "label")?,
                    text: value_text(&text),
                    color,
                    size,
                });
                Ok(())
            },
        );
        // A glTF model from the visual's assets: placed at `at`, turned by
        // `rotate` (degrees), scaled by `scale`, or centered and sized so its
        // longest side is `fit`. Its own materials and textures apply; any
        // material option or `color` given here overrides them.
        methods.add_method_mut("model", |_, s, o: Table| {
            let name = o
                .raw_get::<Option<String>>("asset")?
                .ok_or_else(|| lua_error("s:model needs asset = \"name.glb\""))?;
            let model = s.model(&name)?;
            let at = match o.raw_get::<Value>("at")? {
                Value::Nil => [0.0; 3],
                other => v3(other, "model at")?,
            };
            let mut size = match o.raw_get::<Value>("scale")? {
                Value::Nil => [1.0; 3],
                Value::Integer(n) => [n as f32; 3],
                Value::Number(n) => [n as f32; 3],
                other => v3(other, "model scale")?,
            };
            let (lo, hi) = model.bounds;
            let mut origin = [0.0; 3];
            if let Some(fit) = o.raw_get::<Option<f32>>("fit")? {
                let extent = (0..3).map(|i| hi[i] - lo[i]).fold(1e-6f32, f32::max);
                size = scale(size, fit.abs() / extent);
                origin = scale(add(lo, hi), 0.5);
            }
            let turn = rotation_of(o.raw_get("rotate")?)?;
            let color = scene_color(o.raw_get::<Value>("color")?)?;
            let texture = texture_of(s, &o)?;
            let place = |p: V3| {
                let p = sub(p, origin);
                add(
                    at,
                    rotate(&turn, [p[0] * size[0], p[1] * size[1], p[2] * size[2]]),
                )
            };
            // Normals scale inversely, so a squashed model still shades right.
            let orient = |n: V3| {
                rotate(
                    &turn,
                    normalize([n[0] / size[0], n[1] / size[1], n[2] / size[2]]),
                )
            };
            let mut images: Vec<Option<u32>> = vec![None; model.images.len()];
            for tri in &model.triangles {
                let source = &model.materials[tri.material];
                s.material = material_over(source.material, &o)?;
                s.texture = texture.or_else(|| {
                    let image = source.texture.filter(|i| *i < images.len())?;
                    Some(*images[image].get_or_insert_with(|| {
                        let data = model.images[image].clone();
                        s.texture_index(format!("model:{name}:{image}"), || data)
                    }))
                });
                if tri.uvs.is_none() {
                    s.texture = None;
                }
                let color = color.unwrap_or(Color {
                    token: Token::Rgb(source.rgb),
                    mix: None,
                    alpha: source.alpha,
                });
                let corners = tri.positions.map(place);
                match tri.normals {
                    Some(normals) => s.smooth_face(&corners, &normals.map(orient), color)?,
                    None => s.face(&corners, color, None, source.double_sided)?,
                }
                if let Some(face) = s.faces.last_mut() {
                    face.two_sided = source.double_sided;
                }
                if let Some(uvs) = tri.uvs {
                    s.uv_last(&uvs);
                }
            }
            s.texture = None;
            Ok(())
        });
        // An environment map (an equirectangular image asset, ideally HDR):
        // what metal and glass reflect, and the light around the scene.
        methods.add_method_mut("environment", |_, s, o: Table| {
            let name = o
                .raw_get::<Option<String>>("asset")?
                .ok_or_else(|| lua_error("s:environment needs asset = \"name.hdr\""))?;
            let texture = s.asset_texture(&name)?;
            s.environment = Some(Environment {
                texture,
                intensity: o
                    .raw_get::<Option<f32>>("intensity")?
                    .unwrap_or(1.0)
                    .clamp(0.0, 16.0),
                background: o.raw_get::<Option<bool>>("background")?.unwrap_or(false),
            });
            Ok(())
        });
        // A camera-facing glow (a flare, a glint), `size` pixels across: an
        // image asset, or a soft round light without one.
        methods.add_method_mut("sprite", |_, s, (at, o): (Value, Option<Table>)| {
            s.check_items()?;
            let (at, o) = options_form(at, o)?;
            let at = v3(at, "sprite")?;
            let sprite = match o {
                Some(o) => Sprite {
                    at,
                    size: o
                        .raw_get::<Option<f32>>("size")?
                        .unwrap_or(48.0)
                        .clamp(1.0, 2048.0),
                    texture: texture_of(s, &o)?.or(match o.raw_get::<Option<String>>("asset")? {
                        Some(name) => Some(s.asset_texture(&name)?),
                        None => None,
                    }),
                    color: color_of(&o, Token::Accent)?,
                    glow: o
                        .raw_get::<Option<f32>>("glow")?
                        .unwrap_or(1.0)
                        .clamp(0.0, 16.0),
                },
                None => Sprite {
                    at,
                    size: 48.0,
                    texture: None,
                    color: Color::token(Token::Accent),
                    glow: 1.0,
                },
            };
            s.sprites.push(sprite);
            Ok(())
        });
        // A floor grid on y = 0 in hairlines.
        methods.add_method_mut("grid", |_, s, o: Option<Table>| {
            let (size, step, y) = match &o {
                Some(o) => (
                    o.raw_get::<Option<f32>>("size")?
                        .unwrap_or(10.0)
                        .abs()
                        .max(0.1),
                    o.raw_get::<Option<f32>>("step")?
                        .unwrap_or(1.0)
                        .abs()
                        .max(0.01),
                    o.raw_get::<Option<f32>>("y")?.unwrap_or(0.0),
                ),
                None => (10.0, 1.0, 0.0),
            };
            let half = size / 2.0;
            let n = ((size / step).round() as i32).clamp(1, 200);
            for i in 0..=n {
                s.check_items()?;
                let t = -half + size * i as f32 / n as f32;
                let color = Color::token(Token::Border);
                s.lines.push(Line3 {
                    a: [t, y, -half],
                    b: [t, y, half],
                    color,
                    width: 1.0,
                });
                s.lines.push(Line3 {
                    a: [-half, y, t],
                    b: [half, y, t],
                    color,
                    width: 1.0,
                });
            }
            Ok(())
        });
        // x/y/z axes from the origin, with optional names.
        methods.add_method_mut("axes", |_, s, o: Option<Table>| {
            let size = match &o {
                Some(o) => o.raw_get::<Option<f32>>("size")?.unwrap_or(1.0),
                None => 1.0,
            };
            let names: [String; 3] = match &o {
                Some(o) => match o.raw_get::<Option<Table>>("labels")? {
                    Some(t) => [
                        t.raw_get::<Option<String>>(1)?.unwrap_or_default(),
                        t.raw_get::<Option<String>>(2)?.unwrap_or_default(),
                        t.raw_get::<Option<String>>(3)?.unwrap_or_default(),
                    ],
                    None => ["x".into(), "y".into(), "z".into()],
                },
                None => ["x".into(), "y".into(), "z".into()],
            };
            for (axis, name) in names.into_iter().enumerate() {
                s.check_items()?;
                let mut end = [0.0; 3];
                end[axis] = size;
                s.lines.push(Line3 {
                    a: [0.0; 3],
                    b: end,
                    color: Color::token(Token::Muted),
                    width: 1.0,
                });
                if !name.is_empty() {
                    end[axis] = size * 1.08;
                    s.labels.push(Label3 {
                        at: end,
                        text: name,
                        color: Color::token(Token::Muted),
                        size: TextSize::Xs,
                    });
                }
            }
            Ok(())
        });
    }
}

/// `s:point(at, options)` or `s:point { at = ..., ... }`.
fn options_form(at: Value, o: Option<Table>) -> mlua::Result<(Value, Option<Table>)> {
    if let Value::Table(t) = &at
        && o.is_none()
        && let at_field @ Value::Table(_) = t.raw_get::<Value>("at")?
    {
        return Ok((at_field, Some(t.clone())));
    }
    Ok((at, o))
}

fn line_style(o: Option<Table>) -> mlua::Result<(Color, f32)> {
    match o {
        Some(o) => Ok((
            color_of(&o, Token::Muted)?,
            o.raw_get::<Option<f32>>("width")?
                .unwrap_or(1.0)
                .clamp(0.25, 12.0),
        )),
        None => Ok((Color::token(Token::Muted), 1.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> Scene {
        let mut s = Scene::default();
        let h = 0.5;
        let c = |x: f32, y: f32, z: f32| [x * h, y * h, z * h];
        s.face(
            &[
                c(-1., -1., 1.),
                c(1., -1., 1.),
                c(1., 1., 1.),
                c(-1., 1., 1.),
            ],
            Color::token(Token::Accent),
            Some([0., 0., 1.]),
            false,
        )
        .unwrap();
        s.face(
            &[
                c(1., -1., -1.),
                c(-1., -1., -1.),
                c(-1., 1., -1.),
                c(1., 1., -1.),
            ],
            Color::token(Token::Accent),
            Some([0., 0., -1.]),
            false,
        )
        .unwrap();
        s
    }

    #[test]
    fn the_camera_frames_the_geometry_and_culls_back_faces() {
        let projection = cube().project(
            Camera {
                yaw: 0.0,
                pitch: 0.0,
                zoom: 1.0,
            },
            400.0,
            300.0,
        );
        // Looking down -z from +z: only the +z face is drawn.
        assert_eq!(projection.prims.len(), 1);
        let Prim::Face { points, shade, .. } = &projection.prims[0] else {
            panic!()
        };
        for [x, y] in points {
            assert!(
                (0.0..=400.0).contains(x) && (0.0..=300.0).contains(y),
                "{x},{y}"
            );
        }
        assert!(*shade > 0.35);
        // From behind, the other face.
        let behind = cube().project(
            Camera {
                yaw: std::f32::consts::PI,
                pitch: 0.0,
                zoom: 1.0,
            },
            400.0,
            300.0,
        );
        assert_eq!(behind.prims.len(), 1);
    }

    #[test]
    fn faces_sort_back_to_front() {
        let mut s = Scene::default();
        let near = [[0., 0., 1.], [1., 0., 1.], [1., 1., 1.]];
        let far = [[0., 0., -1.], [1., 0., -1.], [1., 1., -1.]];
        s.face(&near, Color::token(Token::Chart(0)), None, true)
            .unwrap();
        s.face(&far, Color::token(Token::Chart(1)), None, true)
            .unwrap();
        let p = s.project(
            Camera {
                yaw: 0.0,
                pitch: 0.0,
                zoom: 1.0,
            },
            200.0,
            200.0,
        );
        let colors: Vec<Token> = p
            .prims
            .iter()
            .map(|p| match p {
                Prim::Face { color, .. } => color.token,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(colors, [Token::Chart(1), Token::Chart(0)]);
    }

    #[test]
    fn ramps_mix_between_neighbouring_tokens() {
        let colors = [
            Color::token(Token::Chart(0)),
            Color::token(Token::Chart(1)),
            Color::token(Token::Chart(2)),
        ];
        let mid = ramp(&colors, 0.75);
        assert_eq!(mid.token, Token::Chart(1));
        assert_eq!(mid.mix.unwrap().0, Token::Chart(2));
        assert!((mid.mix.unwrap().1 - 0.5).abs() < 1e-5);
    }
}
