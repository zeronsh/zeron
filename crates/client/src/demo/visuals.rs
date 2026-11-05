//! Agent visuals the demo serves (`show_visual` scripts), one of each kind:
//! a dashboard with controls, a pointer-driven canvas, an orbiting scene and
//! a light shader.
//!
//! To preview visuals a desktop rendered instead, point
//! `ZERON_DEMO_VISUALS` at a JSON list of `{ "id", "title", "path" }` (the
//! part's id and title, and its script on disk): the demo chat shows those,
//! reading each script and the files stored beside it.

use std::sync::OnceLock;

use zeron_doc::parts::MessagePart;

#[derive(serde::Deserialize)]
pub(crate) struct Local {
    pub id: String,
    pub title: String,
    pub path: String,
}

/// Visuals listed by `ZERON_DEMO_VISUALS` (empty without it).
pub(crate) fn local() -> &'static [Local] {
    static LOCAL: OnceLock<Vec<Local>> = OnceLock::new();
    LOCAL.get_or_init(|| {
        let Ok(list) = std::env::var("ZERON_DEMO_VISUALS") else {
            return Vec::new();
        };
        std::fs::read(&list)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    })
}

/// A listed visual's script, manifest or one of its assets.
fn local_file(path: &str) -> Option<Vec<u8>> {
    local().iter().find_map(|visual| {
        let stem = visual.path.strip_suffix("-visual.luau");
        let beside = path == visual.path
            || stem.is_some_and(|stem| {
                path == format!("{stem}-assets.json")
                    || path
                        .strip_prefix(&format!("{stem}-asset-"))
                        .is_some_and(|name| !name.contains('/'))
            });
        beside.then(|| std::fs::read(path).ok()).flatten()
    })
}

pub(crate) const DEMO_VISUALS: &[(&str, &str, &str)] = &[
    ("v_demo_dash", "Latency dashboard", DASHBOARD),
    ("v_demo_canvas", "Orbiting particles", CANVAS),
    ("v_demo_scene", "Materials", SCENE),
    ("v_demo_shader", "Light", SHADER),
];

pub(crate) fn path(id: &str) -> String {
    format!("/Users/dev/.zeron/uploads/{id}-visual.luau")
}

/// The file at `path`, if it's a demo visual's.
pub(crate) fn read(path: &str) -> Option<Vec<u8>> {
    DEMO_VISUALS
        .iter()
        .find(|(id, ..)| self::path(id) == path)
        .map(|(_, _, script)| script.as_bytes().to_vec())
        .or_else(|| local_file(path))
}

pub(crate) fn part(index: usize) -> MessagePart {
    let (id, title, _) = DEMO_VISUALS[index];
    MessagePart::Visual {
        id: id.to_owned(),
        path: path(id),
        title: title.to_owned(),
    }
}

const DASHBOARD: &str = r#"
local s = ui.state { range = "7d", table = false, window = 3 }
local days = { "Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun" }
local p95 = { ["7d"] = { 182, 175, 190, 168, 160, 171, 158 }, ["30d"] = { 210, 198, 190, 181, 176, 170, 162 } }
local p50 = { ["7d"] = { 92, 88, 95, 84, 80, 86, 79 }, ["30d"] = { 104, 99, 95, 91, 88, 85, 81 } }
return function()
  local rows = {}
  for i, day in ipairs(days) do
    rows[i] = { day, p95[s.range][i], p50[s.range][i] }
  end
  return ui.column { gap = 14,
    ui.row { gap = 16,
      ui.stat { value = "158 ms", label = "p95 latency", delta = "−8%", tone = "success" },
      ui.stat { value = "99.95%", label = "uptime" },
      ui.stat { value = "4.2k", label = "req / min" },
    },
    ui.tabs { options = { "7d", "30d" }, value = s.range, on_change = function(v) s.range = v end },
    ui.chart { type = "area", height = 160, x = days, unit = "ms",
      series = { { name = "p95", values = p95[s.range] }, { name = "p50", values = p50[s.range] } } },
    ui.slider { label = "Smoothing window", value = s.window, min = 1, max = 7, step = 1,
      on_change = function(v) s.window = v end },
    ui.row { gap = 12, align = "center",
      ui.toggle { label = "Show table", value = s.table, on_change = function(v) s.table = v end },
      ui.button { label = "Reset", on_click = function() s.window = 3 end },
    },
    s.table and ui.table { columns = { "Day", { label = "p95", align = "end" }, { label = "p50", align = "end" } }, rows = rows }
      or ui.space(0),
  }
end
"#;

const CANVAS: &str = r#"
return ui.column { gap = 6,
  ui.text { "Drag across it", size = "sm", color = "muted" },
  ui.canvas { height = 220, animate = true, draw = function(g, w, h, t)
    local p = g.pointer
    local cx = p and p.x or w / 2 + math.cos(t * 0.7) * w / 4
    local cy = p and p.y or h / 2 + math.sin(t * 0.9) * h / 4
    for i = 1, 36 do
      local a = t * 0.8 + i * (math.pi * 2 / 36)
      local r = 34 + 22 * math.sin(t * 2 + i * 0.7)
      g:circle(cx + math.cos(a) * r, cy + math.sin(a) * r, 3.5, { fill = "chart" .. (i % 6 + 1) })
    end
    g:circle(cx, cy, 6, { fill = "accent" })
  end },
}
"#;

const SCENE: &str = r#"
return ui.scene { height = 260, orbit = true, spin = 0.25, draw = function(s)
  s:box { at = { 0, -0.05, 0 }, size = { 5, 0.1, 3.5 }, color = "muted" }
  s:box { at = { -1.4, 0.5, 0 }, size = 1, color = "chart1", roughness = 0.35 }
  s:sphere { at = { 0, 0.6, 0.4 }, radius = 0.6, color = "chart2", metal = 1, roughness = 0.25 }
  s:box { at = { 1.4, 0.6, 0 }, size = { 0.9, 1.2, 0.9 }, color = "chart3", glass = true, dispersion = 1 }
  s:sphere { at = { 1.4, 1.6, -1 }, radius = 0.2, color = "chart4", glow = 4 }
end }
"#;

const SHADER: &str = r#"
return ui.shader { height = 220, bloom = 1.2, code = [[
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
  vec2 uv = (fragCoord - 0.5 * iResolution.xy) / iResolution.y;
  vec2 m = iMouse.x > 0.0 ? (iMouse.xy - 0.5 * iResolution.xy) / iResolution.y : vec2(0.0);
  vec3 col = vec3(0.0);
  for (int i = 0; i < 6; i++) {
    float fi = float(i);
    vec2 c = m + 0.28 * vec2(cos(iTime * 0.9 + fi), sin(iTime * 1.2 + fi * 2.0));
    float d = length(uv - c);
    col += 0.010 / (d * d + 0.002) * (0.5 + 0.5 * cos(6.2831 * (fi / 6.0 + vec3(0.0, 0.33, 0.67))));
  }
  fragColor = vec4(col, 1.0);
}
]] }
"#;

#[cfg(test)]
mod tests {
    #[test]
    fn demo_visuals_pass_the_engine_check() {
        for (id, _, script) in super::DEMO_VISUALS {
            zeron_visual::check(script, zeron_visual::seed_for(id))
                .unwrap_or_else(|e| panic!("{id}: {e}"));
        }
    }
}
