use super::*;

fn text_of(node: &Node) -> String {
    match &node.kind {
        NodeKind::Text(text) => text.spans.iter().map(|s| s.text.as_str()).collect(),
        _ => children(node).map(text_of).collect::<Vec<_>>().join("|"),
    }
}

fn find<'a>(node: &'a Node, pred: &dyn Fn(&Node) -> bool) -> Option<&'a Node> {
    if pred(node) {
        return Some(node);
    }
    children(node).find_map(|child| find(child, pred))
}

#[test]
fn a_view_renders_and_handlers_change_state() {
    let mut visual = Visual::load(
        r#"
        local s = ui.state { tab = "p95" }
        return function()
          return ui.column { gap = 12,
            ui.text { "Latency ", ui.span { s.tab, weight = 600, color = "accent" }, size = "lg" },
            ui.tabs { options = { "p95", "p99" }, value = s.tab, on_change = function(v) s.tab = v end },
          }
        end
        "#,
        1,
    )
    .unwrap();
    let tree = visual.render().unwrap();
    assert_eq!(text_of(&tree), "Latency p95|");
    let tabs = find(&tree, &|n| matches!(n.kind, NodeKind::Tabs { .. })).unwrap();
    let NodeKind::Tabs { on_change, .. } = tabs.kind else {
        unreachable!()
    };
    visual
        .call(on_change, HandlerArg::Text("p99".into()))
        .unwrap();
    assert_eq!(text_of(&visual.render().unwrap()), "Latency p99|");
}

#[test]
fn a_script_may_return_a_static_node() {
    let mut visual = Visual::load(r#"return ui.text "hello""#, 1).unwrap();
    assert_eq!(text_of(&visual.render().unwrap()), "hello");
}

#[test]
fn style_rules_are_enforced_with_reasons() {
    for (script, needle) in [
        (
            r##"return ui.text { "x", color = "#ff0000" }"##,
            "theme tokens",
        ),
        (
            r#"return ui.column { background = "accent" }"#,
            "no containers",
        ),
        (
            r#"return ui.text { "x", font = "Comic Sans" }"#,
            "interface font",
        ),
        (r#"return ui.text { "x", size = "40px" }"#, "type scale"),
        (r#"return ui.row { colour = "x" }"#, "has no 'colour'"),
        (
            r#"return ui.chart { type = "pie", series = {} }"#,
            "ui.canvas",
        ),
        (
            r#"return ui.canvas { draw = function() end }"#,
            "needs height",
        ),
    ] {
        let err = Visual::load(script, 1)
            .and_then(|mut v| v.render())
            .unwrap_err();
        assert!(err.message.contains(needle), "{script}: {}", err.message);
    }
}

#[test]
fn the_sandbox_has_no_io_and_bounds_time_and_memory() {
    for script in [
        "return os.execute('true')",
        "return io.open('/etc/passwd')",
        "return require('x')",
    ] {
        assert!(Visual::load(script, 1).is_err(), "{script}");
    }
    let started = Instant::now();
    let err = Visual::load("while true do end", 1).err().unwrap();
    assert!(err.message.contains("time budget"), "{}", err.message);
    assert!(started.elapsed() < Duration::from_secs(2));

    let err = Visual::load(
        "local t = {} for i = 1, 1e9 do t[i] = string.rep('x', 1024) .. i end",
        1,
    )
    .err()
    .unwrap();
    assert!(
        err.message.contains("MB") || err.message.contains("time budget"),
        "{}",
        err.message
    );
    // The ui library can't be replaced by a script.
    assert!(Visual::load("ui.text = nil return ui.text 'x'", 1).is_err());
}

#[test]
fn random_data_is_stable_across_renders() {
    let script = r#"
        return function() return ui.text { tostring(math.random(1, 1000000)) } end
    "#;
    let mut a = Visual::load(script, 7).unwrap();
    let first = text_of(&a.render().unwrap());
    assert_eq!(first, text_of(&a.render().unwrap()));
    let mut b = Visual::load(script, 7).unwrap();
    assert_eq!(first, text_of(&b.render().unwrap()));
}

#[test]
fn canvases_record_theme_colored_commands() {
    let mut visual = Visual::load(
        r#"
        return ui.canvas { height = 120, draw = function(g, w, h)
          g:line(0, h, w, 0, { color = "border" })
          g:rect(10, 10, 40, 20, { fill = "chart2", radius = 4 })
          g:circle(w / 2, h / 2, 8, { color = ui.color("accent", 0.5) })
          g:arc(60, 60, 30, 0, math.pi, { fill = "chart3", inner = 20 })
          g:text(w, 0, "label", { align = "end", color = "muted", size = "xs" })
        end }
        "#,
        1,
    )
    .unwrap();
    let tree = visual.render().unwrap();
    let NodeKind::Canvas(canvas) = &tree.kind else {
        panic!()
    };
    assert_eq!(canvas.height, 120.0);
    let commands = visual
        .draw_canvas(canvas.draw, 300.0, 120.0, 0.0, None)
        .unwrap();
    assert_eq!(commands.len(), 5);
    assert!(matches!(&commands[0], DrawCmd::Stroke { color, .. } if color.token == Token::Border));
    assert!(matches!(&commands[2], DrawCmd::Fill { color, .. } if color.alpha == 0.5));
    let DrawCmd::Text(text) = &commands[4] else {
        panic!()
    };
    assert_eq!((text.x, text.align), (300.0, Align::End));
}

#[test]
fn scenes_project_lit_depth_sorted_geometry() {
    let mut visual = Visual::load(
        r#"
        return ui.scene { height = 300, spin = 0.3, draw = function(s)
          s:grid { size = 4, step = 1 }
          s:surface { x = { -2, 2 }, z = { -2, 2 }, detail = 12,
            colors = { "chart1", "chart3" },
            f = function(x, z) return math.sin(x) * math.cos(z) end }
          s:box { at = { 0, 1.5, 0 }, size = 0.5, color = "chart2" }
          s:point({ 1, 1, 1 }, { hint = "peak" })
          s:point { at = { -1, 1, -1 }, color = "chart4" }
          s:label { at = { 0, 2, 0 }, text = "top" }
        end }
        "#,
        1,
    )
    .unwrap();
    let tree = visual.render().unwrap();
    let NodeKind::Scene(node) = &tree.kind else {
        panic!()
    };
    let scene = visual.draw_scene(node.draw, 0.0).unwrap();
    assert_eq!(scene.face_count(), 12 * 12 + 6);
    let projection = scene.project(
        Camera {
            yaw: node.yaw,
            pitch: node.pitch,
            zoom: node.zoom,
        },
        640.0,
        300.0,
    );
    assert!(matches!(projection.prims.last(), Some(Prim::Label { .. })));
    assert_eq!(projection.hints.len(), 1);
    // A closed box shows at most three faces.
    let box_faces = projection
        .prims
        .iter()
        .filter(|p| matches!(p, Prim::Face { color, .. } if color.token == Token::Chart(1)))
        .count();
    assert!((1..=3).contains(&box_faces), "{box_faces}");
}

#[test]
fn charts_and_tables_parse() {
    let mut visual = Visual::load(
        r#"
        return ui.column {
          ui.chart { type = "line", height = 200, x = { "Mon", "Tue", "Wed" }, unit = "ms",
            series = { { name = "p95", values = { 120, 180, 140 } }, { name = "p99", values = { 200, nil, 260 } } } },
          ui.table { columns = { "Region", "p95" }, rows = { { "us-east", 182 }, { "eu-west", 140 } } },
          ui.row { ui.stat { value = "182 ms", label = "p95" }, ui.stat { value = 99.2, label = "uptime", tone = "success" } },
        }
        "#,
        1,
    )
    .unwrap();
    let tree = visual.render().unwrap();
    let chart = find(&tree, &|n| matches!(n.kind, NodeKind::Chart(_))).unwrap();
    let NodeKind::Chart(chart) = &chart.kind else {
        unreachable!()
    };
    assert_eq!(chart.series[1].values, [Some(200.0), None, Some(260.0)]);
    assert_eq!(chart.series[1].color, Color::token(Token::Chart(1)));
    let table = find(&tree, &|n| matches!(n.kind, NodeKind::Table(_))).unwrap();
    let NodeKind::Table(table) = &table.kind else {
        unreachable!()
    };
    // Numeric columns right-align.
    assert_eq!(table.columns[1].align, Align::End);
}

#[test]
fn check_exercises_every_control_and_reports_where_it_broke() {
    let report = check(
        r#"
        local s = ui.state { tab = "a", on = false, n = 1 }
        return function()
          return ui.column {
            ui.tabs { options = { "a", "b" }, value = s.tab, on_change = function(v) s.tab = v end },
            ui.text(s.tab),
            ui.toggle { label = "Log scale", value = s.on, on_change = function(v) s.on = v end },
            ui.slider { label = "Window", value = s.n, min = 1, max = 10, on_change = function(v) s.n = v end },
            ui.canvas { height = 100, animate = true, draw = function(g, w, h, t) g:line(0, 0, w * t, h) end },
          }
        end
        "#,
        1,
    )
    .unwrap();
    assert_eq!(report.interactions, 5);

    let err = check(
        r#"
        local s = ui.state { tab = "a" }
        return function()
          local body = s.tab == "a" and ui.text "fine" or ui.text { nil .. "boom" }
          return ui.column {
            ui.tabs { options = { "a", "b" }, value = s.tab, on_change = function(v) s.tab = v end },
            body,
          }
        end
        "#,
        1,
    )
    .unwrap_err();
    assert!(
        err.message.starts_with("after selecting tab 'b':"),
        "{}",
        err.message
    );
    assert!(err.message.contains("visual:4"), "{}", err.message);
}

#[test]
fn ticks_are_round() {
    assert_eq!(
        nice_ticks(0.0, 100.0, 5),
        [0.0, 20.0, 40.0, 60.0, 80.0, 100.0]
    );
    assert_eq!(nice_ticks(0.0, 0.9, 4), [0.0, 0.2, 0.4, 0.6, 0.8]);
    assert_eq!(nice_ticks(5.0, 5.0, 4), [5.0]);
}

#[test]
fn shaders_bind_state_as_inputs_and_fail_with_their_line() {
    let report = check(
        r#"
        local s = ui.state { speed = 1 }
        return function()
          return ui.column {
            ui.slider { label = "Speed", value = s.speed, min = 0, max = 4, on_change = function(v) s.speed = v end },
            ui.shader { height = 240, orbit = true, inputs = { speed = s.speed }, code = [[
              void mainImage(out vec4 fragColor, in vec2 fragCoord) {
                vec2 uv = fragCoord / iResolution.xy;
                fragColor = vec4(uv, 0.5 + 0.5 * sin(iTime * speed + iCamera.x), 1.0);
              }
            ]] },
          }
        end
        "#,
        1,
    )
    .unwrap();
    assert!(report.interactions >= 1);

    let err = check(
        r#"
        return function()
          return ui.shader { height = 200, code = [[
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
  fragColor = vec4(missing, 1.0);
}
          ]] }
        end
        "#,
        1,
    )
    .unwrap_err();
    // Lua drops the newline after `[[`, so mainImage is line 1.
    assert!(err.message.contains("shader line 2:"), "{}", err.message);
}

#[test]
fn shaders_default_to_light_and_keep_the_old_transparent_flag() {
    use crate::shader::Blend;
    let blend_of = |props: &str| {
        let mut visual = Visual::load(
            &format!(
                "return function() return ui.shader {{ height = 100, {props} code = [[void mainImage(out vec4 c, in vec2 p) {{ c = vec4(1.0); }}]] }} end"
            ),
            1,
        )
        .unwrap();
        match visual.render().unwrap().kind {
            NodeKind::Shader(shader) => shader.blend,
            _ => unreachable!(),
        }
    };
    assert_eq!(blend_of(""), Blend::Light);
    assert_eq!(blend_of("transparent = true,"), Blend::Alpha);
    assert_eq!(blend_of("transparent = false,"), Blend::Opaque);
    assert_eq!(blend_of("blend = \"opaque\","), Blend::Opaque);
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    ::image::RgbaImage::from_fn(width, height, |x, y| {
        ::image::Rgba([(x * 30) as u8, (y * 30) as u8, 200, 255])
    })
    .write_to(
        &mut std::io::Cursor::new(&mut out),
        ::image::ImageFormat::Png,
    )
    .unwrap();
    out
}

#[test]
fn scenes_place_models_textures_environments_and_sprites_from_assets() {
    let assets = Assets::new(
        [
            (
                "tri.glb".to_owned(),
                crate::model::tests::triangle_glb().into(),
            ),
            ("wood.png".to_owned(), png(4, 4).into()),
            ("sky.png".to_owned(), png(8, 4).into()),
        ]
        .into_iter()
        .collect(),
    );
    let source = r##"
        return ui.scene { height = 300, draw = function(s)
          s:environment { asset = "sky.png", intensity = 1.5 }
          s:model { asset = "tri.glb", fit = 1, rotate = { 0, 90, 0 } }
          s:model { asset = "tri.glb", at = { 3, 0, 0 }, color = "#00ff00", metal = 1 }
          s:box { at = { 0, -1, 0 }, texture = "wood.png" }
          s:sprite { at = { 0, 2, 0 }, size = 64, color = "#ffeedd", glow = 2 }
        end }
    "##;
    let mut visual = Visual::load(source, 1).unwrap().with_assets(assets.clone());
    let tree = visual.render().unwrap();
    let NodeKind::Scene(node) = &tree.kind else {
        panic!()
    };
    let scene = visual.draw_scene(node.draw, 0.0).unwrap();
    assert_eq!(scene.face_count(), 2 + 6);
    assert_eq!(scene.textures().len(), 2);
    assert_eq!(scene.environment().map(|e| e.intensity), Some(1.5));
    assert_eq!(scene.sprites().len(), 1);
    let faces: Vec<FaceView> = scene.face_views().collect();
    // `fit` centers the model and sizes its longest side; the rotation turns
    // the triangle's plane from z = 0 onto x = 0.
    let fitted = &faces[0];
    for corner in fitted.corners {
        assert!(corner[0].abs() < 1e-5, "{corner:?}");
        assert!(corner[1].abs() <= 0.5 + 1e-5 && corner[2].abs() <= 0.5 + 1e-5);
    }
    assert_eq!(fitted.color.token, Token::Rgb(0xff0000));
    assert!(fitted.two_sided);
    assert_eq!(faces[1].color.token, Token::Rgb(0x00ff00));
    assert_eq!(faces[1].material.metalness, 1.0);
    assert_eq!(faces[1].material.roughness, 0.25);
    let boxed = &faces[2];
    assert_eq!(boxed.texture, Some(1));
    assert_eq!(boxed.uvs.map(|uv| uv.len()), Some(4));
    // The model is parsed once per visual, however often the scene draws.
    visual.draw_scene(node.draw, 1.0).unwrap();
    assert_eq!(assets.models.lock().unwrap().len(), 1);
    check_with_assets(source, 1, assets).unwrap();
}

#[test]
fn missing_and_broken_assets_fail_the_check_with_their_names() {
    let script =
        |body: &str| format!("return ui.scene {{ height = 200, draw = function(s) {body} end }}");
    let error = check(&script(r#"s:model { asset = "prism.glb" }"#), 1).unwrap_err();
    assert!(
        error.message.contains("no asset named 'prism.glb'"),
        "{}",
        error.message
    );
    let assets = Assets::new(
        [
            ("bad.png".to_owned(), b"nope".to_vec().into()),
            ("bad.glb".to_owned(), b"nope".to_vec().into()),
        ]
        .into_iter()
        .collect(),
    );
    let error = check_with_assets(
        &script(r#"s:box { at = {0,0,0}, texture = "bad.png" }"#),
        1,
        assets.clone(),
    )
    .unwrap_err();
    assert!(error.message.contains("bad.png"), "{}", error.message);
    let error =
        check_with_assets(&script(r#"s:model { asset = "bad.glb" }"#), 1, assets).unwrap_err();
    assert!(
        error.message.contains("model 'bad.glb'"),
        "{}",
        error.message
    );
}

/// Select each tab of the first tabs control and return what the view says.
fn tab_texts(script: &str) -> Vec<String> {
    let mut visual = Visual::load(script, 1).unwrap();
    let tree = visual.render().unwrap();
    let tabs = find(&tree, &|n| matches!(n.kind, NodeKind::Tabs { .. })).unwrap();
    let NodeKind::Tabs { options, .. } = tabs.kind.clone() else {
        unreachable!()
    };
    let mut out = vec![text_of(&tree)];
    for option in options {
        let tree = visual.render().unwrap();
        let tabs = find(&tree, &|n| matches!(n.kind, NodeKind::Tabs { .. })).unwrap();
        let NodeKind::Tabs { on_change, .. } = tabs.kind else {
            unreachable!()
        };
        visual.call(on_change, HandlerArg::Text(option)).unwrap();
        out.push(text_of(&visual.render().unwrap()));
    }
    out
}

#[test]
fn nil_children_leave_holes_instead_of_ending_the_list() {
    // `cond and node or nil` per tab: before, everything after the first
    // unselected tab's nil was dropped, so only the first tab ever showed.
    let script = r#"
        local s = ui.state { tab = "A" }
        return function()
          return ui.column {
            ui.tabs { options = { "A", "B", "C" }, value = s.tab, on_change = function(v) s.tab = v end },
            s.tab == "A" and ui.text "showing A" or nil,
            s.tab == "B" and ui.text "showing B" or nil,
            s.tab == "C" and ui.text "showing C" or nil,
          }
        end
    "#;
    assert_eq!(
        tab_texts(script),
        ["|showing A", "|showing A", "|showing B", "|showing C"]
    );
    assert_eq!(check(script, 1).unwrap().tabs, 1);

    let mut visual = Visual::load(
        r#"return ui.column { ui.text { "a", nil, ui.span { "c" } }, ui.table { columns = { "x", "y", "z" }, rows = { { 1, nil, 3 } } } }"#,
        1,
    )
    .unwrap();
    let tree = visual.render().unwrap();
    assert_eq!(text_of(&tree), "ac|1||3", "the nil cell stays a blank cell");
}

#[test]
fn tabs_take_positions_and_label_value_options() {
    let by_position = r#"
        local s = ui.state { tab = 1 }
        local names = { "A", "B" }
        return function()
          return ui.column {
            ui.tabs { options = names, value = s.tab, on_change = function(i) s.tab = i end },
            ui.text("showing " .. names[s.tab]),
          }
        end
    "#;
    assert_eq!(
        tab_texts(by_position),
        ["|showing A", "|showing A", "|showing B"]
    );
    check(by_position, 1).unwrap();

    let keyed = r#"
        local s = ui.state { tab = "a" }
        return function()
          return ui.column {
            ui.tabs { options = { { label = "Alpha", value = "a" }, { "Beta", "b" } }, value = s.tab,
              on_change = function(v) s.tab = v end },
            ui.text("showing " .. s.tab),
          }
        end
    "#;
    let mut visual = Visual::load(keyed, 1).unwrap();
    let tree = visual.render().unwrap();
    let tabs = find(&tree, &|n| matches!(n.kind, NodeKind::Tabs { .. })).unwrap();
    let NodeKind::Tabs { options, value, .. } = &tabs.kind else {
        unreachable!()
    };
    assert_eq!(
        (options.as_slice(), value.as_str()),
        (&["Alpha".to_owned(), "Beta".to_owned()][..], "Alpha")
    );
    assert_eq!(tab_texts(keyed), ["|showing a", "|showing a", "|showing b"]);
    check(keyed, 1).unwrap();
}

#[test]
fn check_rejects_tabs_that_dont_switch_the_view() {
    for (script, needle) in [
        // The view keys on different strings than the options.
        (
            r#"local s = ui.state { tab = "Overview" }
            return function() return ui.column {
              ui.tabs { options = { "Overview", "Details" }, value = s.tab, on_change = function(v) s.tab = v end },
              s.tab == "Overview" and ui.text "o" or nil,
              s.tab == "details" and ui.text "d" or nil,
            } end"#,
            "compares the tab state with \"details\"",
        ),
        // A misspelt key with more tabs: two of them show only the tabs.
        (
            r#"local s = ui.state { tab = "Day" }
            return function() return ui.column {
              ui.tabs { options = { "Day", "Week", "Month" }, value = s.tab, on_change = function(v) s.tab = v end },
              s.tab == "Day" and ui.text "d" or nil,
              s.tab == "Weekly" and ui.text "w" or nil,
              s.tab == "Monthly" and ui.text "m" or nil,
            } end"#,
            "show the same view",
        ),
        // The state starts nil, so nothing shows until a click.
        (
            r#"local s = ui.state {}
            return function() return ui.column {
              ui.tabs { options = { "A", "B" }, value = s.tab, on_change = function(v) s.tab = v end },
              s.tab == "A" and ui.text "a" or nil,
              s.tab == "B" and ui.text "b" or nil,
            } end"#,
            "is the tab state nil at first",
        ),
        // The handler never stores the choice.
        (
            r#"local s = ui.state { tab = "A" }
            return function() return ui.column {
              ui.tabs { options = { "A", "B" }, value = s.tab, on_change = function(v) end },
              ui.text(s.tab),
            } end"#,
            "still show 'A' selected",
        ),
    ] {
        let error = check(script, 1).unwrap_err();
        assert!(
            error.message.contains(needle),
            "{needle}: {}",
            error.message
        );
    }
}

#[test]
fn check_accepts_tabs_that_change_only_a_drawing_or_a_nested_tab() {
    let canvas = r#"
        local s = ui.state { tab = "red" }
        return function() return ui.column {
          ui.tabs { options = { "red", "blue" }, value = s.tab, on_change = function(v) s.tab = v end },
          ui.canvas { height = 80, draw = function(g, w, h)
            g:rect(0, 0, s.tab == "red" and 10 or 20, 10, { fill = "accent" })
          end },
        } end
    "#;
    assert_eq!(check(canvas, 1).unwrap().tabs, 1);

    let nested = r#"
        local s = ui.state { outer = "A", inner = "x" }
        return function() return ui.column {
          ui.tabs { options = { "A", "B" }, value = s.outer, on_change = function(v) s.outer = v end },
          s.outer == "B" and ui.tabs { options = { "x", "y" }, value = s.inner, on_change = function(v) s.inner = v end } or nil,
          ui.text(s.outer .. s.inner),
        } end
    "#;
    assert_eq!(check(nested, 1).unwrap().tabs, 2, "the tabs inside tab B");

    // Switching tabs also clears a selection the first view started with.
    let resets = r#"
        local s = ui.state { tab = "A", picked = "x" }
        return function() return ui.column {
          ui.tabs { options = { "A", "B", "B" }, value = s.tab,
            on_change = function(v) s.tab = v; s.picked = "none" end },
          ui.text(s.tab .. " " .. s.picked),
        } end
    "#;
    assert_eq!(check(resets, 1).unwrap().tabs, 1);
}
