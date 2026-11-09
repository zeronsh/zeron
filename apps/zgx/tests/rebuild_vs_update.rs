//! What a growing paragraph costs a frame when its row is built again for
//! each new end (as ds/markdown's blocks are) against when its text is
//! updated in place (a span bound to the text): the potential of updating a
//! streaming block in place. Instructions a frame by phase (`ZGX_PROFILE`).
//! A probe, not a check:
//!
//!     ZGX_PROFILE=1 cargo test --release --test rebuild_vs_update -- --ignored --nocapture

use zgx_script::{ScriptHost, Value, testing::TestApp};

const SCRIPT: &str = r#"
local text = ui.shared("probe.text", "")
local rebuild = ui.shared("probe.rebuild", true)
local list = ui.computed(function()
    -- Some settled paragraphs, then the one growing.
    local out = {}
    for i = 1, 12 do
        table.insert(out, { id = i, t = "Settled paragraph " .. i .. " with a few words that wrap across the width of the column, so that each takes two lines or so of the reply." })
    end
    table.insert(out, { id = 13, t = text:get() })
    return out
end)
return ui.column {
    w = 684,
    gap = 16,
    ui.show(function() return rebuild:get() end, ui.each(list, function(item)
        return ui.div { w_full = true, ui.rich { wrap = true, ui.span { item.t, literal = true } } }
    end)),
    ui.show(function() return not rebuild:get() end, ui.each(list, function(item, _, row)
        return ui.div { w_full = true, ui.rich { wrap = true, ui.span { function() return row:get().t end, literal = true } } }
    end, { key = function(item) return item.id end })),
}
"#;

fn run(rebuild: bool) -> Vec<(String, f64)> {
    let app = TestApp::inline(SCRIPT);
    app.ui.scene.borrow_mut().resize(900., 700.);
    let text = app.host.shared("probe.text", Value::from(""));
    app.host.shared("probe.rebuild", Value::from(rebuild));
    app.frames(2);
    let mut gpu = zgx_gpu::GpuRenderer::new(900, 700).expect("a GPU");
    let damage = app.ui.scene.borrow_mut().flush().damage;
    gpu.render(&app.ui.scene.borrow(), &damage).unwrap();
    let words = "The growing paragraph takes its words a few at a time, as a reply streams in, and wraps as it goes. ".repeat(20);
    let mut totals: std::collections::BTreeMap<String, u64> = Default::default();
    let mut frames = 0;
    for end in (24..words.len()).step_by(24) {
        zgx::profile::take();
        text.set(Value::from(&words[..end]));
        app.frames(1);
        let damage = app.ui.scene.borrow_mut().flush().damage;
        gpu.render(&app.ui.scene.borrow(), &damage).unwrap();
        let taken = zgx::profile::take();
        if end > 600 {
            frames += 1;
            for (phase, instructions, _) in &taken {
                *totals.entry(format!("{phase:?}")).or_default() += instructions;
            }
        }
        // Layout a frame by how long the growing paragraph is.
        if rebuild && end % 240 == 0 {
            let of = |name: &str| -> u64 { taken.iter().filter(|(p, ..)| format!("{p:?}") == name).map(|(_, i, _)| *i).sum() };
            eprintln!("  at {end} chars: layout {}K paint {}K encode {}K", of("Layout") / 1000, of("Paint") / 1000, of("Encode") / 1000);
        }
    }
    assert_eq!(app.error(), None);
    let mut app = app;
    eprintln!("shown: {}", app.find("~\"The growing paragraph\"").is_some());
    eprintln!("{}", app.tree().lines().take(12).collect::<Vec<_>>().join("\n"));
    totals.into_iter().map(|(p, i)| (p, i as f64 / frames as f64)).collect()
}

#[test]
#[ignore = "a probe: ZGX_PROFILE=1, release, --ignored --nocapture"]
fn a_growing_paragraph_rebuilt_against_updated() {
    assert!(zgx::profile::enabled(), "run with ZGX_PROFILE=1");
    for (name, rebuild) in [("rebuilt each chunk", true), ("updated in place", false)] {
        let phases = run(rebuild);
        let total: f64 = phases.iter().map(|(_, i)| i).sum();
        eprintln!(
            "{name}: {:.0}K instr a frame: {}",
            total / 1e3,
            phases.iter().filter(|(_, i)| *i > 0.).map(|(p, i)| format!("{p} {:.0}K", i / 1e3)).collect::<Vec<_>>().join(", ")
        );
    }
}
