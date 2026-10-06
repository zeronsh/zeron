use super::tests::composer_focus_window;
use super::*;
use zeron_proto::attachment_mentions::{attachment_mention_indices, attachment_mention_link};

fn png() -> gpui::Image {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbImage::new(1, 1)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    gpui::Image::from_bytes(gpui::ImageFormat::Png, bytes.into_inner())
}

fn paste_images(
    handle: &gpui::WindowHandle<Composer>,
    cx: &mut gpui::TestAppContext,
    count: usize,
) {
    handle
        .update(cx, |composer, _, cx| {
            composer.input.update(cx, |_, cx| {
                cx.emit(ComposerInputEvent::PastedImages(
                    (0..count).map(|_| png()).collect(),
                ))
            });
        })
        .unwrap();
    cx.run_until_parked();
}

/// Replace a range of the draft the way typing would, so each edit reaches the
/// composer as an ordinary input edit.
fn edit(
    handle: &gpui::WindowHandle<Composer>,
    cx: &mut gpui::TestAppContext,
    range_and_text: impl FnOnce(&ComposerInput) -> (Range<usize>, String),
) {
    handle
        .update(cx, |composer, window, cx| {
            composer.input.update(cx, |input, cx| {
                let (range, new_text) = range_and_text(input);
                let range = input.range_to_utf16(&range);
                input.replace_text_in_range(Some(range), &new_text, window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
}

fn append(handle: &gpui::WindowHandle<Composer>, cx: &mut gpui::TestAppContext, new_text: &str) {
    let new_text = new_text.to_string();
    edit(handle, cx, move |input| {
        let end = input.text().len();
        (end..end, new_text)
    });
}

fn text(handle: &gpui::WindowHandle<Composer>, cx: &mut gpui::TestAppContext) -> String {
    handle
        .read_with(cx, |composer, cx| {
            composer.input.read(cx).text().to_string()
        })
        .unwrap()
}

fn staged_names(
    handle: &gpui::WindowHandle<Composer>,
    cx: &mut gpui::TestAppContext,
) -> Vec<String> {
    handle
        .read_with(cx, |composer, _| {
            composer
                .staged()
                .iter()
                .map(|att| att.name.clone())
                .collect()
        })
        .unwrap()
}

#[gpui::test]
fn pasting_images_numbers_them_and_mentions_each_at_the_caret(cx: &mut gpui::TestAppContext) {
    let (_dir, handle) = composer_focus_window(cx);
    handle
        .update(cx, |composer, _, cx| {
            composer
                .input
                .update(cx, |input, cx| input.set_text("compare ", cx));
        })
        .unwrap();
    paste_images(&handle, cx, 2);
    assert_eq!(staged_names(&handle, cx), ["Image 1.png", "Image 2.png"]);
    assert_eq!(
        text(&handle, cx),
        format!(
            "compare {} {} ",
            attachment_mention_link(1, None),
            attachment_mention_link(2, None)
        )
    );
    handle
        .read_with(cx, |composer, cx| {
            let display = &composer.input.read(cx).projection.display;
            assert!(
                display.contains(&format!("{CHIP_ICON_SLOT}Image\u{a0}1")),
                "{display:?}"
            );
            assert_eq!(composer.input.read(cx).projection.mentions.len(), 2);
        })
        .unwrap();
}

#[gpui::test]
fn one_undo_removes_a_paste_and_redo_restores_it(cx: &mut gpui::TestAppContext) {
    let (_dir, handle) = composer_focus_window(cx);
    paste_images(&handle, cx, 2);
    handle
        .update(cx, |composer, window, cx| {
            composer
                .input
                .update(cx, |input, cx| input.undo(&Undo, window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(text(&handle, cx), "");
    assert!(staged_names(&handle, cx).is_empty());
    handle
        .update(cx, |composer, window, cx| {
            composer
                .input
                .update(cx, |input, cx| input.redo(&Redo, window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(attachment_mention_indices(&text(&handle, cx)), vec![1, 2]);
    assert_eq!(staged_names(&handle, cx), ["Image 1.png", "Image 2.png"]);
}

#[gpui::test]
fn deleting_the_last_chip_unstages_and_undo_restores_the_image(cx: &mut gpui::TestAppContext) {
    let (_dir, handle) = composer_focus_window(cx);
    paste_images(&handle, cx, 2);
    handle
        .update(cx, |composer, _, cx| {
            composer
                .input
                .update(cx, |input, cx| input.remove_attachment_chips(1, cx));
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(staged_names(&handle, cx), ["Image 2.png"]);
    handle
        .update(cx, |composer, window, cx| {
            composer
                .input
                .update(cx, |input, cx| input.undo(&Undo, window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(staged_names(&handle, cx), ["Image 1.png", "Image 2.png"]);
}

#[gpui::test]
fn numbers_are_never_reused_after_a_removal(cx: &mut gpui::TestAppContext) {
    let (_dir, handle) = composer_focus_window(cx);
    paste_images(&handle, cx, 2);
    handle
        .update(cx, |composer, _, cx| {
            composer
                .input
                .update(cx, |input, cx| input.remove_attachment_chips(2, cx));
        })
        .unwrap();
    cx.run_until_parked();
    paste_images(&handle, cx, 1);
    assert_eq!(staged_names(&handle, cx), ["Image 1.png", "Image 3.png"]);
}

#[gpui::test]
fn removing_a_thumbnail_removes_every_chip_for_it(cx: &mut gpui::TestAppContext) {
    let (_dir, handle) = composer_focus_window(cx);
    paste_images(&handle, cx, 1);
    append(
        &handle,
        cx,
        &format!(" and {}", attachment_mention_link(1, None)),
    );
    assert_eq!(attachment_mention_indices(&text(&handle, cx)), vec![1]);
    assert_eq!(staged_names(&handle, cx).len(), 1);
    handle
        .update(cx, |composer, _, cx| {
            let id = composer.staged()[0].id.clone();
            composer.remove_attachment(&id, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(attachment_mention_indices(&text(&handle, cx)).is_empty());
    assert!(!text(&handle, cx).contains("zeron-image"));
    assert!(staged_names(&handle, cx).is_empty());
}

#[gpui::test]
fn a_chip_may_repeat_and_the_image_stays_until_the_last_is_removed(cx: &mut gpui::TestAppContext) {
    let (_dir, handle) = composer_focus_window(cx);
    paste_images(&handle, cx, 1);
    append(
        &handle,
        cx,
        &format!(" again {}", attachment_mention_link(1, None)),
    );
    for expected_left in [1usize, 0] {
        edit(&handle, cx, |input| {
            let first = zeron_proto::attachment_mentions::attachment_mentions(input.text())[0]
                .range
                .clone();
            (first, String::new())
        });
        assert_eq!(staged_names(&handle, cx).len(), expected_left);
    }
}

#[gpui::test]
fn attachments_without_chips_are_left_alone(cx: &mut gpui::TestAppContext) {
    let (_dir, handle) = composer_focus_window(cx);
    handle
        .update(cx, |composer, _, cx| {
            let key = composer.current_key.clone();
            composer
                .attachments
                .entry(key)
                .or_default()
                .push(crate::attachments::stage_clipboard_image(png()));
            composer
                .input
                .update(cx, |input, cx| input.set_text("hello", cx));
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(staged_names(&handle, cx), ["image.png"]);
}

fn tiled_names(
    handle: &gpui::WindowHandle<Composer>,
    cx: &mut gpui::TestAppContext,
) -> Vec<String> {
    handle
        .read_with(cx, |composer, cx| {
            composer
                .tiled_attachments(cx)
                .iter()
                .map(|att| att.name.clone())
                .collect()
        })
        .unwrap()
}

#[gpui::test]
fn only_attachments_without_a_chip_get_a_tile(cx: &mut gpui::TestAppContext) {
    let (_dir, handle) = composer_focus_window(cx);
    paste_images(&handle, cx, 2);
    // The chips stand in for the staged images: no tile repeats them.
    assert_eq!(staged_names(&handle, cx).len(), 2);
    assert!(tiled_names(&handle, cx).is_empty());
    handle
        .update(cx, |composer, _, _| {
            let key = composer.current_key.clone();
            composer
                .attachments
                .entry(key)
                .or_default()
                .push(crate::attachments::stage_clipboard_image(png()));
        })
        .unwrap();
    cx.run_until_parked();
    // One with no chip keeps its tile, the only handle it has.
    assert_eq!(tiled_names(&handle, cx), ["image.png"]);
}

#[gpui::test]
fn undoing_a_thumbnail_removal_restores_image_and_chip(cx: &mut gpui::TestAppContext) {
    let (_dir, handle) = composer_focus_window(cx);
    paste_images(&handle, cx, 1);
    handle
        .update(cx, |composer, _, cx| {
            let id = composer.staged()[0].id.clone();
            composer.remove_attachment(&id, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(staged_names(&handle, cx).is_empty());
    handle
        .update(cx, |composer, window, cx| {
            composer
                .input
                .update(cx, |input, cx| input.undo(&Undo, window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(staged_names(&handle, cx), ["Image 1.png"]);
    assert_eq!(attachment_mention_indices(&text(&handle, cx)), vec![1]);
}

fn add_file(
    handle: &gpui::WindowHandle<Composer>,
    cx: &mut gpui::TestAppContext,
    path: std::path::PathBuf,
) {
    handle
        .update(cx, |composer, _, cx| composer.add_paths(vec![path], cx))
        .unwrap();
    cx.run_until_parked();
}

fn write_file(dir: &tempfile::TempDir, name: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, b"# notes").unwrap();
    path
}

#[gpui::test]
fn a_file_attachment_is_staged_with_a_chip_named_after_it(cx: &mut gpui::TestAppContext) {
    let (dir, handle) = composer_focus_window(cx);
    add_file(&handle, cx, write_file(&dir, "notes.md"));
    assert_eq!(staged_names(&handle, cx), ["notes.md"]);
    assert_eq!(
        text(&handle, cx),
        format!("{} ", attachment_mention_link(1, Some("notes.md")))
    );
    handle
        .read_with(cx, |composer, cx| {
            let input = composer.input.read(cx);
            assert_eq!(input.projection.mentions.len(), 1);
            assert_eq!(input.projection.mentions[0].0.kind, ChipKind::File);
            assert!(input.projection.display.contains("notes.md"));
            assert_eq!(composer.staged()[0].bytes(), b"# notes");
        })
        .unwrap();
}

#[gpui::test]
fn images_and_files_share_one_numbering(cx: &mut gpui::TestAppContext) {
    let (dir, handle) = composer_focus_window(cx);
    paste_images(&handle, cx, 1);
    add_file(&handle, cx, write_file(&dir, "notes.md"));
    paste_images(&handle, cx, 1);
    assert_eq!(
        staged_names(&handle, cx),
        ["Image 1.png", "notes.md", "Image 3.png"]
    );
    assert_eq!(
        attachment_mention_indices(&text(&handle, cx)),
        vec![1, 2, 3]
    );
}

#[gpui::test]
fn deleting_a_file_chip_removes_the_file_and_undo_restores_it(cx: &mut gpui::TestAppContext) {
    let (dir, handle) = composer_focus_window(cx);
    add_file(&handle, cx, write_file(&dir, "notes.md"));
    handle
        .update(cx, |composer, _, cx| {
            composer
                .input
                .update(cx, |input, cx| input.remove_attachment_chips(1, cx));
        })
        .unwrap();
    cx.run_until_parked();
    assert!(staged_names(&handle, cx).is_empty());
    handle
        .update(cx, |composer, window, cx| {
            composer
                .input
                .update(cx, |input, cx| input.undo(&Undo, window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(staged_names(&handle, cx), ["notes.md"]);
}

#[gpui::test]
fn at_completion_offers_staged_files_by_name(cx: &mut gpui::TestAppContext) {
    let (dir, handle) = composer_focus_window(cx);
    paste_images(&handle, cx, 1);
    add_file(&handle, cx, write_file(&dir, "notes.md"));
    append(&handle, cx, " @note");
    handle
        .update(cx, |composer, _, cx| {
            let rows = composer.mention_attachment_matches();
            assert_eq!(
                rows.iter()
                    .map(|row| row.label.as_str())
                    .collect::<Vec<_>>(),
                ["notes.md"]
            );
            assert!(rows[0].image.is_none());
            composer.accept_mention(cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        attachment_mention_indices(&text(&handle, cx)),
        vec![1, 2],
        "the file chip is mentioned a second time"
    );
    assert!(
        text(&handle, cx)
            .trim_end()
            .ends_with(&attachment_mention_link(2, Some("notes.md")))
    );
}

#[gpui::test]
fn a_folder_is_refused_without_staging(cx: &mut gpui::TestAppContext) {
    let (dir, handle) = composer_focus_window(cx);
    add_file(&handle, cx, dir.path().to_path_buf());
    assert!(staged_names(&handle, cx).is_empty());
    assert_eq!(text(&handle, cx), "");
    handle
        .read_with(cx, |composer, _| assert!(composer.failure().is_some()))
        .unwrap();
}

#[gpui::test]
fn a_chip_without_a_staged_image_shows_as_plain_text(cx: &mut gpui::TestAppContext) {
    let (_dir, handle) = composer_focus_window(cx);
    handle
        .update(cx, |composer, _, cx| {
            composer.input.update(cx, |input, cx| {
                input.set_text(format!("see {}", attachment_mention_link(4, None)), cx)
            });
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .read_with(cx, |composer, cx| {
            let input = composer.input.read(cx);
            assert!(input.projection.mentions.is_empty());
            assert!(input.projection.display.contains("Image 4"));
            assert!(!input.projection.display.contains('@'));
        })
        .unwrap();
}

#[gpui::test]
fn images_without_chips_still_count_as_content_and_send_state(cx: &mut gpui::TestAppContext) {
    let (_dir, handle) = composer_focus_window(cx);
    paste_images(&handle, cx, 1);
    handle
        .read_with(cx, |composer, cx| {
            assert!(composer.has_draft(cx));
            assert!(composer_has_content(
                composer.input.read(cx).text(),
                composer.staged().len(),
                0
            ));
        })
        .unwrap();
}

#[gpui::test]
fn at_completion_pins_staged_images_above_files_and_filters_them(cx: &mut gpui::TestAppContext) {
    let (_dir, handle) = composer_focus_window(cx);
    paste_images(&handle, cx, 2);
    append(&handle, cx, " @");
    handle
        .read_with(cx, |composer, _| {
            let matches = composer.mention_attachment_matches();
            assert_eq!(
                matches.iter().map(|row| row.index).collect::<Vec<_>>(),
                vec![1, 2]
            );
            assert_eq!(composer.mention.active, Some(0));
        })
        .unwrap();
    append(&handle, cx, "image2");
    handle
        .update(cx, |composer, _, cx| {
            let matches = composer.mention_attachment_matches();
            assert_eq!(
                matches.iter().map(|row| row.index).collect::<Vec<_>>(),
                vec![2]
            );
            composer.accept_mention(cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        text(&handle, cx),
        format!(
            "{} {}  {} ",
            attachment_mention_link(1, None),
            attachment_mention_link(2, None),
            attachment_mention_link(2, None)
        )
    );
    assert_eq!(staged_names(&handle, cx), ["Image 1.png", "Image 2.png"]);
}

#[gpui::test]
fn at_completion_has_no_image_rows_when_nothing_is_staged(cx: &mut gpui::TestAppContext) {
    let (_dir, handle) = composer_focus_window(cx);
    handle
        .update(cx, |composer, _, cx| {
            composer
                .input
                .update(cx, |input, cx| input.set_text("@", cx));
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .read_with(cx, |composer, _| {
            assert!(composer.mention_attachment_matches().is_empty());
        })
        .unwrap();
}

#[gpui::test]
fn chips_copied_from_another_draft_paste_as_plain_labels(cx: &mut gpui::TestAppContext) {
    let (_dir, handle) = composer_focus_window(cx);
    handle
        .update(cx, |composer, window, cx| {
            let link = attachment_mention_link(1, None);
            composer.input.update(cx, |input, cx| {
                input.attachment_scope = "chat-a".into();
                cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
                    "Image 1".into(),
                    serde_json::json!({
                        "zeronComposerV1": link,
                        "text": "Image 1",
                        "zeronAttachmentScope": "chat-a",
                    }),
                ));
                input.attachment_scope = "chat-b".into();
                input.paste(&Paste, window, cx);
            });
        })
        .unwrap();
    assert_eq!(text(&handle, cx), "Image 1");
    handle
        .update(cx, |composer, window, cx| {
            composer.input.update(cx, |input, cx| {
                input.set_text("", cx);
                input.attachment_scope = "chat-a".into();
                input.paste(&Paste, window, cx);
            });
        })
        .unwrap();
    assert_eq!(text(&handle, cx), attachment_mention_link(1, None));
}

#[test]
fn sent_messages_project_image_chips_for_the_transcript() {
    let raw = format!("look at {} please", attachment_mention_link(2, None));
    let (display, spans) = sent_mention_display(&raw).expect("image chips project");
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].kind, ChipKind::Image);
    assert_eq!(
        &display[spans[0].range.clone()],
        format!("{MENTION_SIDE_PAD}{CHIP_ICON_SLOT}Image\u{a0}2{CHIP_TRAILING_PAD}")
    );
    assert!(sent_mention_display("Image 2 without a link").is_none());
    assert!(sent_mention_display("[Image 2](zeron-image:9)").is_none());
}

#[test]
fn file_chips_use_the_file_theme_to_tell_formats_apart() {
    use crate::theme::Appearance::Dark;
    let themed = |kind, name| match chip_icon(kind, name, Dark) {
        ChipIcon::FileTheme(path) => path,
        ChipIcon::Glyph(_) => panic!("{name} should use the file theme"),
    };
    assert_ne!(
        themed(ChipKind::File, "README.md"),
        themed(ChipKind::File, "src/main.rs")
    );
    assert_ne!(
        themed(ChipKind::File, "logo.png"),
        themed(ChipKind::File, "src/lib.rs")
    );
    assert_ne!(
        themed(ChipKind::File, "src/lib.rs"),
        themed(ChipKind::Directory, "src/")
    );
    assert!(matches!(
        chip_icon(ChipKind::Image, "", Dark),
        ChipIcon::Glyph(_)
    ));
}

/// Put a queued message carrying `Image 1` and `notes.md` into the composer,
/// as an acquired edit does.
fn begin_queued_edit(
    handle: &gpui::WindowHandle<Composer>,
    cx: &mut gpui::TestAppContext,
    dir: &tempfile::TempDir,
) {
    let notes = crate::attachments::stage_file(&write_file(dir, "notes.md")).unwrap();
    let image = crate::attachments::stage_png_bytes("ab12cd34-Image_1.png".into(), Vec::new());
    let text = format!(
        "summarize {} and {}",
        attachment_mention_link(1, None),
        attachment_mention_link(2, Some("notes.md"))
    );
    handle
        .update(cx, |composer, _, cx| {
            composer.editing_queued = Some("row".into());
            composer.swap_in_queued_draft(text, vec![image, notes], Vec::new(), cx);
        })
        .unwrap();
    cx.run_until_parked();
}

fn remove_chip(handle: &gpui::WindowHandle<Composer>, cx: &mut gpui::TestAppContext, index: u32) {
    edit(handle, cx, move |input| {
        let chip = zeron_proto::attachment_mentions::attachment_mentions(input.text())
            .into_iter()
            .find(|chip| chip.index == index)
            .unwrap();
        (chip.range, String::new())
    });
}

#[gpui::test]
fn a_queued_edit_keeps_its_chips_live_and_deleting_one_unstages_it(cx: &mut gpui::TestAppContext) {
    let (dir, handle) = composer_focus_window(cx);
    begin_queued_edit(&handle, cx, &dir);
    // The restored attachments (named as uploaded) take the numbers of the
    // chips naming them.
    assert_eq!(
        staged_names(&handle, cx),
        ["ab12cd34-Image_1.png", "notes.md"]
    );
    handle
        .read_with(cx, |composer, cx| {
            assert_eq!(composer.input.read(cx).projection.mentions.len(), 2);
        })
        .unwrap();

    // A file attached during the edit is numbered after the message's chips.
    add_file(&handle, cx, write_file(&dir, "logs.zip"));
    assert_eq!(
        staged_names(&handle, cx),
        ["ab12cd34-Image_1.png", "notes.md", "logs.zip"]
    );
    assert_eq!(
        attachment_mention_indices(&text(&handle, cx)),
        vec![1, 2, 3]
    );

    // Deleting its chip unstages it, so saving would not upload it...
    remove_chip(&handle, cx, 3);
    assert_eq!(
        staged_names(&handle, cx),
        ["ab12cd34-Image_1.png", "notes.md"]
    );
    // ...and undo brings it back.
    handle
        .update(cx, |composer, window, cx| {
            composer
                .input
                .update(cx, |input, cx| input.undo(&Undo, window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        staged_names(&handle, cx),
        ["ab12cd34-Image_1.png", "notes.md", "logs.zip"]
    );

    // A restored file's chip deletes the same way, and the saved text never
    // carries a chip for an attachment that is not saved with it.
    remove_chip(&handle, cx, 2);
    append(
        &handle,
        cx,
        &format!(" {}", attachment_mention_link(9, None)),
    );
    assert_eq!(
        staged_names(&handle, cx),
        ["ab12cd34-Image_1.png", "logs.zip"]
    );
    handle
        .read_with(cx, |composer, cx| {
            let saved = composer.queue_edit_text(cx);
            assert_eq!(attachment_mention_indices(&saved), vec![1, 3]);
            assert!(saved.ends_with(" Image 9"), "{saved:?}");
        })
        .unwrap();
}

#[gpui::test]
fn leaving_a_queued_edit_restores_the_draft_and_its_numbering(cx: &mut gpui::TestAppContext) {
    let (dir, handle) = composer_focus_window(cx);
    handle
        .update(cx, |composer, _, cx| {
            composer
                .input
                .update(cx, |input, cx| input.set_text("draft ", cx));
        })
        .unwrap();
    paste_images(&handle, cx, 1);
    let draft = text(&handle, cx);

    begin_queued_edit(&handle, cx, &dir);
    add_file(&handle, cx, write_file(&dir, "logs.zip"));
    remove_chip(&handle, cx, 3);
    handle
        .update(cx, |composer, _, cx| composer.clear_queue_edit_local(cx))
        .unwrap();
    cx.run_until_parked();

    // The draft comes back with its own image and live chip...
    assert_eq!(text(&handle, cx), draft);
    assert_eq!(staged_names(&handle, cx), ["Image 1.png"]);
    handle
        .read_with(cx, |composer, cx| {
            assert_eq!(composer.input.read(cx).projection.mentions.len(), 1);
        })
        .unwrap();
    // ...and its own numbering: the edit's numbers never leak into it.
    paste_images(&handle, cx, 1);
    assert_eq!(staged_names(&handle, cx), ["Image 1.png", "Image 2.png"]);
}

/// Press on the first chip and release `drag` away from where it went down.
fn click_first_chip(
    handle: &gpui::WindowHandle<Composer>,
    cx: &mut gpui::TestAppContext,
    drag: Point<Pixels>,
) {
    cx.update_window((*handle).into(), |_, window, cx| window.draw(cx).clear())
        .unwrap();
    handle
        .update(cx, |composer, window, cx| {
            composer.input.update(cx, |input, cx| {
                let at = input.mention_hits[0].bounds.center();
                input.on_mouse_down(
                    &MouseDownEvent {
                        button: MouseButton::Left,
                        position: at,
                        click_count: 1,
                        ..Default::default()
                    },
                    window,
                    cx,
                );
                input.on_mouse_up(
                    &MouseUpEvent {
                        button: MouseButton::Left,
                        position: at + drag,
                        click_count: 1,
                        ..Default::default()
                    },
                    window,
                    cx,
                );
            });
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui::test]
fn clicking_an_image_chip_opens_the_picture_full_size(cx: &mut gpui::TestAppContext) {
    let (_dir, handle) = composer_focus_window(cx);
    paste_images(&handle, cx, 1);
    // Hovering shows no card: the click is how the picture is seen.
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
        .unwrap();
    handle
        .update(cx, |composer, _, cx| {
            composer.input.update(cx, |input, cx| {
                let at = input.mention_hits[0].bounds.center();
                input.on_mention_pointer_move(at, cx);
                assert_eq!(input.mention_tooltip, MentionTooltipPhase::Hidden);
            });
        })
        .unwrap();

    // A drag across the chip only selects.
    click_first_chip(&handle, cx, point(px(40.0), px(0.0)));
    handle
        .read_with(cx, |composer, _| assert!(composer.preview.is_none()))
        .unwrap();

    click_first_chip(&handle, cx, point(px(0.0), px(0.0)));
    handle
        .read_with(cx, |composer, _| {
            let preview = composer.preview.as_ref().expect("the chip opens its image");
            assert_eq!(preview.name.as_ref(), "Image 1.png");
        })
        .unwrap();
}
