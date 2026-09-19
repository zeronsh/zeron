//! Isolated native evidence for production completion controls and rich composer.
use gpui::{
    AppContext, AsyncApp, Bounds, Context, Entity, Focusable, Render, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, size,
};
use std::{ops::Range, path::PathBuf, time::Duration};
use zeron_theme::SurfacePreference;
use zeron_ui::*;

struct Fixture {
    composer: Entity<composer::Composer>,
    agents: Entity<settings::harnesses::HarnessesPage>,
    settings: bool,
    title: &'static str,
}
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::Theme::of(cx).clone();
        div()
            .id("fixture")
            .size_full()
            // The production shell owns focus traversal. This isolated host
            // repeats that behavior so dispatched Tab keystrokes exercise the
            // composer's real focus handles and keyboard listeners.
            .on_key_down(|event, window, cx| {
                let key = &event.keystroke;
                if key.key == "tab"
                    && !key.modifiers.control
                    && !key.modifiers.alt
                    && !key.modifiers.platform
                {
                    if key.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    cx.stop_propagation();
                }
            })
            .bg(theme.surface)
            .text_color(theme.text)
            .font_family(theme.font_sans.clone())
            .p(px(24.0))
            .overflow_y_scroll()
            .child(if self.settings {
                div()
                    .flex()
                    .flex_col()
                    .child(settings::widgets::page_header(&theme, "Agents", None))
                    .child(
                        self.agents
                            .update(cx, |page, cx| page.fixture_completion(cx)),
                    )
                    .into_any_element()
            } else {
                div()
                    .h_full()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(px(20.0))
                    .child(settings::widgets::page_header(&theme, self.title, None))
                    .child(self.composer.clone())
                    .into_any_element()
            })
    }
}

struct DraftCase {
    name: &'static str,
    title: &'static str,
    text: String,
    selection: Range<usize>,
}
impl DraftCase {
    fn at(name: &'static str, title: &'static str, text: &str, caret_after: &str) -> Self {
        let caret = text.find(caret_after).unwrap() + caret_after.len();
        Self {
            name,
            title,
            text: text.to_owned(),
            selection: caret..caret,
        }
    }
}

fn markdown_cases() -> Vec<DraftCase> {
    let bullets = "Before the list\n- First item\n- Second item\n- Third item\nAfter the list";
    let inline = "**Bold phrase** then text\n_Italic phrase_ and `inline code`\nContinue here";
    let quoted = "> - Quoted item\n>   - Nested café 日本語\n> - [x] Finished quoted task\n> - [ ] Pending quoted task\n> ```rust\n> /* multiline\n>    comment */\n> let emoji = \"👩🏽‍💻\";\n> ```\n- Outside the quote\nContinue here";
    let crlf = "Heading with setext\r\n-\r\n\r\n- First Windows line\r\n- [x] Finished Windows task\r\n- [ ] Pending Windows task\r\n-\r\nContinue here";
    let unicode_file = zeron_proto::file_mentions::local_file_link("src/日本語/café.rs", false);
    let unicode_skill = zeron_proto::invocation::Invocation::Skill {
        name: "review-unicode".into(),
        path: "/project/.agents/skills/review-unicode/SKILL.md".into(),
        command: None,
    }
    .link();
    let unicode = format!(
        "- Review café, naïve, 日本語, e\u{301} and 👩🏽‍💻 beside {unicode_file} and {unicode_skill} while this sentence wraps across several rows.\n- Keep selection and chips aligned.\nContinue here"
    );
    vec![
        DraftCase::at(
            "bullets-before",
            "Bullets · editing preceding paragraph",
            bullets,
            "Before the list",
        ),
        DraftCase::at(
            "bullets-after",
            "Bullets · editing following paragraph",
            bullets,
            "After the list",
        ),
        DraftCase::at(
            "bullets-first",
            "Bullets · editing first item",
            bullets,
            "First item",
        ),
        DraftCase::at(
            "bullets-middle",
            "Bullets · editing middle item",
            bullets,
            "Second item",
        ),
        DraftCase::at(
            "bullets-last",
            "Bullets · editing last item",
            bullets,
            "Third item",
        ),
        DraftCase::at(
            "lists",
            "Ordered, nested, task and empty items",
            "1. Ordered item\n2. Another item\n   - Nested item\n- [x] Finished task\n- [ ] Pending task\n-\nContinue here",
            "Continue here",
        ),
        DraftCase::at(
            "tasks-active",
            "Task marker while editing its content",
            "- [x] Finished task\n- [ ] Pending task\n- Ordinary bullet\nContinue here",
            "Pending task",
        ),
        DraftCase::at(
            "inline-active",
            "Inline formatting while editing its content",
            inline,
            "Bold phrase",
        ),
        DraftCase {
            name: "inline-selection",
            title: "Selection across formatted lines",
            text: inline.to_owned(),
            selection: 2..inline.find(" and ").unwrap(),
        },
        DraftCase::at(
            "markdown",
            "Headings, quotes and inline formatting",
            "# Heading\n## Subheading\n> Quoted **strong text**\n**Bold** and _italic_ and ~~removed~~\nInline `code()` and café 日本語\n---\nContinue here",
            "Continue here",
        ),
        DraftCase::at(
            "code",
            "Code stays literal while surrounding prose renders",
            "**Outside code** and ``one ` tick``\n```rust\nlet source = \"**literal**\";\n// - no bullet here\n```\n- Render this item\nContinue here",
            "Continue here",
        ),
        DraftCase::at(
            "quoted-lists",
            "Quoted lists, tasks and multiline code",
            quoted,
            "Continue here",
        ),
        DraftCase::at(
            "quoted-task-active",
            "Editing a quoted task",
            quoted,
            "Pending quoted task",
        ),
        DraftCase::at(
            "crlf-lists",
            "Windows line endings and empty items",
            crlf,
            "Continue here",
        ),
        DraftCase::at(
            "unicode-wrap",
            "Unicode and chips across wrapped rows",
            &unicode,
            "👩🏽‍💻",
        ),
        DraftCase {
            name: "unicode-wrap-selection",
            title: "Selection across Unicode and chips",
            selection: unicode.find("café").unwrap()..unicode.find(" while this sentence").unwrap(),
            text: unicode,
        },
    ]
}

async fn pause(cx: &mut AsyncApp) {
    pause_for(cx, 400).await;
}

async fn pause_for(cx: &mut AsyncApp, milliseconds: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(milliseconds))
        .await;
}

async fn wheel_to_bottom(
    window: gpui::WindowHandle<Fixture>,
    target: &'static str,
    visible_top: bool,
    output: &std::path::Path,
    filename: String,
    cx: &mut AsyncApp,
) {
    let (position, before, max) = window
        .update(cx, |view, _, cx| {
            view.composer
                .read(cx)
                .fixture_scroll_probe(target, visible_top)
        })
        .unwrap();
    assert!(
        max > px(0.0),
        "{target} fixture must overflow before its wheel probe"
    );
    let capture_window: gpui::AnyWindowHandle = window.into();
    capture_window
        .update(cx, |_, window, cx| {
            window.dispatch_event(
                gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                    position,
                    delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-10_000.0))),
                    modifiers: gpui::Modifiers::default(),
                    touch_phase: gpui::TouchPhase::Moved,
                }),
                cx,
            );
        })
        .unwrap();
    pause(cx).await;
    capture_window
        .update(cx, |_, window, cx| {
            window.draw(cx).clear();
            window
                .render_to_image()
                .unwrap()
                .save(output.join(filename))
                .unwrap();
        })
        .unwrap();
    let (_, after, settled_max) = window
        .update(cx, |view, _, cx| {
            view.composer
                .read(cx)
                .fixture_scroll_probe(target, visible_top)
        })
        .unwrap();
    assert!(
        after < before,
        "{target} wheel event did not move its scroller"
    );
    assert!(
        (after + settled_max).abs() <= px(1.0),
        "{target} wheel event did not reach bottom: offset={after:?}, max={settled_max:?}"
    );
}

fn fixture_surface() -> anyhow::Result<(SurfacePreference, &'static str)> {
    match std::env::var("ZERON_FIXTURE_SURFACE")
        .unwrap_or_else(|_| "frosted".into())
        .to_ascii_lowercase()
        .as_str()
    {
        "frosted" => Ok((SurfacePreference::Frosted, "frosted")),
        "opaque" => Ok((SurfacePreference::Opaque, "opaque")),
        value => {
            anyhow::bail!("invalid ZERON_FIXTURE_SURFACE={value:?}; expected `frosted` or `opaque`")
        }
    }
}

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let (surface, surface_name) = fixture_surface()?;
    let reduced_motion = std::env::var_os("ZERON_FIXTURE_REDUCE_MOTION").is_some();
    let motion_name = if reduced_motion {
        "reduced"
    } else {
        "standard"
    };
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
        gpui_tokio::init(cx); gpui_base::init(cx);
        let mut prefs = settings::UiSettings::default();
        prefs.compact_model_picker = true;
        prefs.surface = surface;
        settings::init(prefs.clone(), data.clone(), cx);
        motion::set_reduced_motion(cx, reduced_motion);
        let fonts = typography::register_fonts(cx);
        typography::init(prefs.ui_font_family.clone(), prefs.ui_font_size, prefs.terminal_font_family.clone(), prefs.terminal_font_size, prefs.code_font_family.clone(), prefs.code_font_size, fonts, cx);
        theme_library::init(data.clone(), cx);
        appearance::init(appearance::AppearanceMode::Dark, prefs.theme_selection, prefs.accent, prefs.surface, cx);
        composer::init(cx, prefs.composer_send_behavior);
        let state = cx.new(|_| state::AppState::new());
        let composer = cx.new(|cx| composer::Composer::new(state.clone(), cx));
        let agents = cx.new(|cx| settings::harnesses::HarnessesPage::new(state.clone(), cx));
        let skill = zeron_proto::invocation::Invocation::Skill { name: "review-changes".into(), path: "/project/.agents/skills/review-changes/SKILL.md".into(), command: None }.link();
        let file = zeron_proto::file_mentions::local_file_link("src/composer.rs", false);
        let long_file = zeron_proto::file_mentions::local_file_link("src/components/very-long-internationalized-component-name.test.tsx", false);
        let draft = format!("Review {file} with {skill}\nAlso check {long_file}\n**Keep the layout calm** and _easy to edit_.\n- Preserve keyboard navigation\n- Check café and 日本語\n```rust\nlet chips = render(&draft);\n```\nContinue here");
        composer.update(cx, |view, cx| view.fixture_rich_draft(&draft, cx));
        let window = cx.open_window(WindowOptions { window_bounds: Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(40.), px(40.)), size(px(840.), px(960.))))), ..Default::default() }, |_, cx| cx.new(|_| Fixture { composer, agents, settings: true, title: "Composer" })).unwrap();
        cx.activate(true);
        cx.spawn(async move |cx| {
            for light in [false, true] {
                cx.update(|cx| appearance::set_mode(if light { appearance::AppearanceMode::Light } else { appearance::AppearanceMode::Dark }, cx));
                for settings in [true, false] {
                    for width in [840., 440.] {
                        window.update(cx, |view, w, cx| {
                            view.settings = settings;
                            w.resize(size(px(width), px(960.)));
                            if !settings {
                                w.activate_window();
                                w.focus(&view.composer.focus_handle(cx), cx);
                            }
                            cx.notify();
                        }).unwrap();
                        pause(cx).await;
                        let name = format!("{}-{}-{}.png", if settings { "agents" } else { "composer" }, if light { "light" } else { "dark" }, width as u32);
                        let capture_window: gpui::AnyWindowHandle = window.into();
                        capture_window.update(cx, |_, w, cx| { w.draw(cx).clear(); w.render_to_image().unwrap().save(output.join(name)).unwrap(); }).unwrap();
                    }
                }
            }
            // Each editing case gets a wide dark and narrow light capture. The
            // shared composer case above covers the complementary combinations.
            for case in markdown_cases() {
                for (light, width) in [(false, 840.), (true, 440.)] {
                    cx.update(|cx| appearance::set_mode(if light { appearance::AppearanceMode::Light } else { appearance::AppearanceMode::Dark }, cx));
                    window.update(cx, |view, w, cx| {
                        view.settings = false;
                        view.title = case.title;
                        view.composer.update(cx, |composer, cx| composer.fixture_rich_selection(&case.text, case.selection.clone(), cx));
                        w.resize(size(px(width), px(960.)));
                        w.activate_window();
                        w.focus(&view.composer.focus_handle(cx), cx);
                        cx.notify();
                    }).unwrap();
                    pause(cx).await;
                    let name = format!("{}-{}-{}.png", case.name, if light { "light" } else { "dark" }, width as u32);
                    let capture_window: gpui::AnyWindowHandle = window.into();
                    capture_window.update(cx, |_, w, cx| { w.draw(cx).clear(); w.render_to_image().unwrap().save(output.join(name)).unwrap(); }).unwrap();
                    if case.name == "bullets-after" && !light {
                        // Exercise production cursor navigation without resetting
                        // the draft or explicitly refreshing its projection.
                        for (name, up, count) in [("bullets-arrow-up", true, 3), ("bullets-arrow-down", false, 1)] {
                            for _ in 0..count {
                                capture_window.update(cx, |_, w, cx| {
                                    if up { w.dispatch_action(Box::new(composer::Up), cx); }
                                    else { w.dispatch_action(Box::new(composer::Down), cx); }
                                }).unwrap();
                                pause(cx).await;
                            }
                            window.update(cx, |view, _, cx| {
                                view.title = if up { "Bullets · after ArrowUp navigation" } else { "Bullets · after ArrowDown navigation" };
                                cx.notify();
                            }).unwrap();
                            let name = format!("{name}-dark-840.png");
                            capture_window.update(cx, |_, w, cx| { w.draw(cx).clear(); w.render_to_image().unwrap().save(output.join(name)).unwrap(); }).unwrap();
                        }
                    }
                }
            }
            let cases: serde_json::Value = serde_json::from_str(include_str!("../../../scripts/fixtures/composer-questions.json")).unwrap();
            for case in cases.as_array().unwrap() {
                for (light, width) in [(false, 840.), (true, 440.)] {
                    let question = serde_json::from_value(case["question"].clone()).unwrap();
                    cx.update(|cx| appearance::set_mode(if light { appearance::AppearanceMode::Light } else { appearance::AppearanceMode::Dark }, cx));
                    window.update(cx, |view, w, cx| {
                        view.settings = false;
                        view.title = "Agent question";
                        view.composer.update(cx, |composer, cx| composer.fixture_question(question, cx));
                        w.resize(size(px(width), px(960.)));
                        cx.notify();
                    }).unwrap();
                    pause(cx).await;
                    let name = format!("question-{}-{}-{}.png", case["name"].as_str().unwrap(), if light { "light" } else { "dark" }, width as u32);
                    let capture_window: gpui::AnyWindowHandle = window.into();
                    capture_window.update(cx, |_, w, cx| { w.draw(cx).clear(); w.render_to_image().unwrap().save(output.join(name)).unwrap(); }).unwrap();
                }
            }
            let question_cases = cases.as_array().unwrap();
            let paginated = vec![
                serde_json::from_value(question_cases.iter().find(|case| case["name"] == "text-only").unwrap()["question"].clone()).unwrap(),
                serde_json::from_value(question_cases.iter().find(|case| case["name"] == "multiple-with-text").unwrap()["question"].clone()).unwrap(),
            ];
            window.update(cx, |view, w, cx| {
                view.settings = false;
                view.title = "Agent question · page 2 of 2";
                view.composer.update(cx, |composer, cx| composer.fixture_paginated_question(paginated, cx));
                w.resize(size(px(440.), px(520.)));
                cx.notify();
            }).unwrap();
            pause(cx).await;
            let capture_window: gpui::AnyWindowHandle = window.into();
            capture_window.update(cx, |_, w, cx| { w.draw(cx).clear(); w.render_to_image().unwrap().save(output.join("question-pagination-page-2.png")).unwrap(); }).unwrap();
            window.update(cx, |view, _, cx| {
                view.title = "Agent question · restored page 1 of 2";
                view.composer.update(cx, |composer, cx| composer.fixture_question_back(cx));
                cx.notify();
            }).unwrap();
            pause(cx).await;
            capture_window.update(cx, |_, w, cx| { w.draw(cx).clear(); w.render_to_image().unwrap().save(output.join("question-pagination-page-1-restored.png")).unwrap(); }).unwrap();
            let cases: serde_json::Value = serde_json::from_str(include_str!("../../../scripts/fixtures/composer-activity.json")).unwrap();
            for case in cases.as_array().unwrap() {
                for expanded in [false, true] {
                    window.update(cx, |view, w, cx| {
                        view.title = "Agent activity";
                        w.resize(size(px(840.), px(960.)));
                        let calls = serde_json::from_value(case["calls"].clone()).unwrap();
                        view.composer.update(cx, |composer, cx| composer.fixture_activity(calls, expanded, cx));
                        cx.notify();
                    }).unwrap();
                    pause(cx).await;
                    let name = format!("activity-{}-{}.png", case["name"].as_str().unwrap(), if expanded { "expanded" } else { "collapsed" });
                    let capture_window: gpui::AnyWindowHandle = window.into();
                    capture_window.update(cx, |_, w, cx| { w.draw(cx).clear(); w.render_to_image().unwrap().save(output.join(name)).unwrap(); }).unwrap();
                }
            }
            // The densest supported stack: activity, a pending question, and
            // queued follow-ups in a short/narrow window. This catches trays
            // disappearing or consuming the answer controls under pressure.
            let activity_cases: serde_json::Value = serde_json::from_str(include_str!("../../../scripts/fixtures/composer-activity.json")).unwrap();
            let dense_calls: Vec<zeron_proto::ToolCall> = serde_json::from_value(
                activity_cases.as_array().unwrap().iter().find(|case| case["name"] == "combined").unwrap()["calls"].clone()
            ).unwrap();
            let question_cases: serde_json::Value = serde_json::from_str(include_str!("../../../scripts/fixtures/composer-questions.json")).unwrap();
            let dense_question: zeron_proto::UserInputQuestion = serde_json::from_value(
                question_cases.as_array().unwrap().iter().find(|case| case["name"] == "long-content").unwrap()["question"].clone()
            ).unwrap();
            for (light, width, height, size_name) in [
                (false, 440., 520., "min"),
                (true, 440., 520., "min"),
                (false, 840., 960., "normal"),
                (true, 840., 960., "normal"),
            ] {
                cx.update(|cx| appearance::set_mode(if light { appearance::AppearanceMode::Light } else { appearance::AppearanceMode::Dark }, cx));
                window.update(cx, |view, w, cx| {
                    view.settings = false;
                    view.title = "Activity, question and queue";
                    view.composer.update(cx, |composer, cx| {
                        composer.fixture_activity(dense_calls.clone(), true, cx);
                        composer.fixture_queue(&[
                            "Run the focused checks after this answer",
                            "Summarize any remaining provider-specific gaps",
                            "Prepare the review notes without losing this queue",
                        ], cx);
                        composer.fixture_question(dense_question.clone(), cx);
                    });
                    w.resize(size(px(width), px(height)));
                    cx.notify();
                }).unwrap();
                pause(cx).await;
                window.update(cx, |view, _, cx| {
                    assert!(
                        view.composer
                            .read(cx)
                            .fixture_has_pending_question("fixture-question", cx),
                        "dense-stack capture requires its durable pending question"
                    );
                }).unwrap();
                let capture_window: gpui::AnyWindowHandle = window.into();
                let name = format!(
                    "dense-stack-{}-{surface_name}-{size_name}.png",
                    if light { "light" } else { "dark" }
                );
                capture_window.update(cx, |_, w, cx| { w.draw(cx).clear(); w.render_to_image().unwrap().save(output.join(name)).unwrap(); }).unwrap();
            }

            // Prove each constrained tray's hidden content is reachable via
            // its actual GPUI wheel path. Isolating the trays prevents a
            // later-painted sibling from intercepting the event coordinate.
            window.update(cx, |view, w, cx| {
                view.settings = false;
                view.title = "Activity · wheel at bottom";
                view.composer.update(cx, |composer, cx| {
                    composer.fixture_activity(dense_calls.clone(), true, cx);
                });
                w.resize(size(px(440.), px(520.)));
                cx.notify();
            }).unwrap();
            pause(cx).await;
            wheel_to_bottom(
                window,
                "activity",
                false,
                &output,
                format!("scroll-activity-bottom-{surface_name}.png"),
                cx,
            ).await;

            window.update(cx, |view, w, cx| {
                view.title = "Queue · wheel at bottom";
                view.composer.update(cx, |composer, cx| {
                    composer.fixture_activity(Vec::new(), false, cx);
                    composer.fixture_queue(&[
                        "First queued follow-up",
                        "Second queued follow-up",
                        "Third queued follow-up",
                        "Fourth queued follow-up",
                        "Fifth queued follow-up",
                        "Sixth queued follow-up",
                        "Seventh queued follow-up",
                        "Eighth queued follow-up",
                    ], cx);
                });
                w.resize(size(px(440.), px(520.)));
                cx.notify();
            }).unwrap();
            pause(cx).await;
            wheel_to_bottom(
                window,
                "queue",
                false,
                &output,
                format!("scroll-queue-bottom-{surface_name}.png"),
                cx,
            ).await;

            window.update(cx, |view, w, cx| {
                view.title = "Question · wheel at bottom";
                view.composer.update(cx, |composer, cx| {
                    composer.fixture_activity(Vec::new(), false, cx);
                    composer.fixture_question(dense_question.clone(), cx);
                });
                w.resize(size(px(440.), px(520.)));
                cx.notify();
            }).unwrap();
            pause(cx).await;
            wheel_to_bottom(
                window,
                "question",
                false,
                &output,
                format!("scroll-question-bottom-{surface_name}.png"),
                cx,
            ).await;

            // Repeat through the complete overlapping stack. Reset before
            // each wheel so every assertion proves that target's visible hit
            // area receives the event instead of inheriting another offset.
            for target in ["activity", "queue", "question"] {
                window.update(cx, |view, w, cx| {
                    view.title = "Dense stack · wheel reachability";
                    view.composer.update(cx, |composer, cx| {
                        composer.fixture_activity(dense_calls.clone(), true, cx);
                        composer.fixture_queue(&[
                            "Run the focused checks after this answer",
                            "Summarize any remaining provider-specific gaps",
                            "Prepare the review notes without losing this queue",
                        ], cx);
                        composer.fixture_question(dense_question.clone(), cx);
                    });
                    w.resize(size(px(440.), px(520.)));
                    cx.notify();
                }).unwrap();
                pause(cx).await;
                wheel_to_bottom(
                    window,
                    target,
                    true,
                    &output,
                    format!("scroll-dense-{target}-bottom-{surface_name}.png"),
                    cx,
                ).await;
            }

            // Use actual keystrokes against the same disclosure rendered in
            // production. The bounded Tab loop proves it remains reachable as
            // focus order evolves; Enter, Escape, and Space exercise its own
            // keyboard listener rather than mutating expansion directly.
            cx.update(|cx| appearance::set_mode(appearance::AppearanceMode::Dark, cx));
            window.update(cx, |view, w, cx| {
                view.settings = false;
                view.title = "Activity · keyboard and motion";
                view.composer.update(cx, |composer, cx| {
                    composer.fixture_activity(dense_calls.clone(), false, cx);
                });
                w.resize(size(px(440.), px(520.)));
                w.activate_window();
                w.focus(&view.composer.focus_handle(cx), cx);
                cx.notify();
            }).unwrap();
            pause(cx).await;
            let capture_window: gpui::AnyWindowHandle = window.into();
            let mut tab_count = 0;
            let activity_focused = loop {
                let focused = window.update(cx, |view, w, cx| {
                    view.composer.read(cx).fixture_activity_keyboard_state(w).0
                }).unwrap();
                if focused || tab_count == 32 {
                    break focused;
                }
                capture_window.update(cx, |_, w, cx| {
                    assert!(w.dispatch_keystroke(gpui::Keystroke::parse("tab").unwrap(), cx));
                }).unwrap();
                tab_count += 1;
            };
            assert!(activity_focused, "activity disclosure was not reachable after {tab_count} Tab keystrokes");
            capture_window.update(cx, |_, w, cx| { w.draw(cx).clear(); w.render_to_image().unwrap().save(output.join(format!("keyboard-activity-tab-focus-{surface_name}.png"))).unwrap(); }).unwrap();

            // Dispatch without borrowing the root Fixture: key handlers may
            // update it while routing focus and actions through the window.
            capture_window.update(cx, |_, w, cx| {
                assert!(w.dispatch_keystroke(gpui::Keystroke::parse("enter").unwrap(), cx));
            }).unwrap();
            window.update(cx, |view, w, cx| {
                assert!(view.composer.read(cx).fixture_activity_keyboard_state(w).1);
            }).unwrap();
            capture_window.update(cx, |_, w, cx| { w.draw(cx).clear(); w.render_to_image().unwrap().save(output.join(format!("activity-motion-{motion_name}-000ms-{surface_name}.png"))).unwrap(); }).unwrap();
            pause_for(cx, 90).await;
            capture_window.update(cx, |_, w, cx| { w.draw(cx).clear(); w.render_to_image().unwrap().save(output.join(format!("activity-motion-{motion_name}-090ms-{surface_name}.png"))).unwrap(); }).unwrap();
            pause_for(cx, 160).await;
            capture_window.update(cx, |_, w, cx| { w.draw(cx).clear(); w.render_to_image().unwrap().save(output.join(format!("activity-motion-{motion_name}-250ms-{surface_name}.png"))).unwrap(); }).unwrap();

            capture_window.update(cx, |_, w, cx| {
                assert!(w.dispatch_keystroke(gpui::Keystroke::parse("escape").unwrap(), cx));
            }).unwrap();
            window.update(cx, |view, w, cx| {
                assert!(!view.composer.read(cx).fixture_activity_keyboard_state(w).1);
            }).unwrap();
            capture_window.update(cx, |_, w, cx| {
                assert!(w.dispatch_keystroke(gpui::Keystroke::parse("space").unwrap(), cx));
            }).unwrap();
            window.update(cx, |view, w, cx| {
                assert!(view.composer.read(cx).fixture_activity_keyboard_state(w).1);
            }).unwrap();
            pause(cx).await;
            capture_window.update(cx, |_, w, cx| { w.draw(cx).clear(); w.render_to_image().unwrap().save(output.join(format!("keyboard-activity-space-expanded-{surface_name}.png"))).unwrap(); }).unwrap();

            for (models, fast, name) in [(false, false, "compact-standard"), (false, true, "compact-fast"), (true, false, "compact-favorites")] {
                window.update(cx, |view, w, cx| {
                    view.composer.update(cx, |composer, cx| {
                        composer.fixture_activity(Vec::new(), false, cx);
                        composer.fixture_compact_picker(w, models, fast, cx);
                    });
                    cx.notify();
                }).unwrap();
                pause(cx).await;
                let capture_window: gpui::AnyWindowHandle = window.into();
                capture_window.update(cx, |_, w, cx| { w.draw(cx).clear(); w.render_to_image().unwrap().save(output.join(format!("{name}.png"))).unwrap(); }).unwrap();
            }
            cx.update(|cx| cx.quit());
        }).detach();
    });
    Ok(())
}
