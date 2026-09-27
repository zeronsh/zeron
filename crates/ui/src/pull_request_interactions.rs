use super::*;
use crate::composer::ComposerInputEvent;
use gpui::Focusable;

impl PullRequestDetailPage {
    pub(super) fn comment_event(&mut self, event: &ComposerInputEvent, cx: &mut Context<Self>) {
        match event {
            ComposerInputEvent::Submitted | ComposerInputEvent::ModifiedSubmitted => {
                self.send_comment(cx)
            }
            ComposerInputEvent::Edited | ComposerInputEvent::CursorMoved => {
                let input = self.comment_input.read(cx);
                self.mention_token =
                    crate::composer::mention_token(input.text(), input.cursor_offset());
                self.mention_choices.clear();
                if let (Some(token), Some(detail)) = (&self.mention_token, &self.detail) {
                    let query = token.query.to_lowercase();
                    self.mention_choices = std::iter::once(&detail.author.login)
                        .chain(
                            detail
                                .comments
                                .iter()
                                .chain(&detail.reviews)
                                .map(|comment| &comment.author.login),
                        )
                        .filter(|login| !login.is_empty() && login.to_lowercase().contains(&query))
                        .cloned()
                        .collect();
                    self.mention_choices.sort();
                    self.mention_choices.dedup();
                    self.mention_choices.truncate(8);
                }
                self.mention_index = 0;
                self.sync_mention_controls(cx);
            }
            ComposerInputEvent::MentionNavigate(delta) => {
                if !self.mention_choices.is_empty() {
                    self.mention_index = (self.mention_index as isize + delta)
                        .rem_euclid(self.mention_choices.len() as isize)
                        as usize;
                }
            }
            ComposerInputEvent::MentionAccept => self.accept_person(cx),
            ComposerInputEvent::MentionDismiss => {
                self.mention_token = None;
                self.mention_choices.clear();
                self.sync_mention_controls(cx);
            }
            _ => {}
        }
        cx.notify();
    }

    fn sync_mention_controls(&mut self, cx: &mut Context<Self>) {
        let open = self.mention_token.is_some();
        let selected = !self.mention_choices.is_empty();
        self.comment_input.update(cx, |input, cx| {
            input.set_mention_controls(open, selected, cx)
        });
    }

    fn accept_person(&mut self, cx: &mut Context<Self>) {
        if let (Some(token), Some(login)) = (
            self.mention_token.take(),
            self.mention_choices.get(self.mention_index).cloned(),
        ) {
            self.comment_input.update(cx, |input, cx| {
                input.replace_plain_token(token.range, &format!("@{login}"), cx)
            });
        }
        self.mention_choices.clear();
        self.sync_mention_controls(cx);
    }

    fn send_comment(&mut self, cx: &mut Context<Self>) {
        let body = self.comment_input.read(cx).text().to_owned();
        if self.comment_task.is_some() || self.detail.is_none() || body.trim().is_empty() {
            return;
        }
        if body.len() > 60_000 {
            self.comment_error = Some("Comment is too long (maximum 60,000 bytes).".into());
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.comment_error = Some("Connect to the PR’s device to send your comment.".into());
            return;
        };
        self.task = None;
        self.loading = false;
        self.comment_error = None;
        let mut params = self.params(false);
        params["body"] = body.clone().into();
        self.comment_task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::POST_CHANGE_REQUEST_COMMENT, params).await
                .map_err(|error| format!("Could not confirm submission: {error}. Your draft is kept; check GitHub before sending again."))
                .and_then(|value| serde_json::from_value::<zeron_proto::ChangeRequestComment>(value).map_err(|error| error.to_string()));
            let result = cx.background_executor().spawn(async move {
                result.map(|comment| { let parsed = super::super::pull_request_media::parse_description(&comment.body); (comment, parsed) })
            }).await;
            let _ = this.update(cx, |page, cx| {
                page.comment_task = None;
                match result {
                    Ok((comment, parsed)) => {
                        if let Some(detail) = &mut page.detail {
                            page.activity_bodies.insert(detail.comments.len(), parsed);
                            detail.comments.push(comment);
                        }
                        if page.comment_input.read(cx).text() == body {
                            page.comment_input.update(cx, |input, cx| input.set_text("", cx));
                        }
                        if let (Some(detail), Some(parsed)) = (&page.detail, &page.body) {
                            let snapshot = DetailSnapshot { detail: detail.clone(), body: parsed.clone(), activity: page.activity_bodies.clone(), fetched: page.fetched.unwrap_or_else(Instant::now), diff: page.diff_snapshot() };
                            page.cache.borrow_mut().put(page.target.clone(), page.url.clone(), snapshot);
                        }
                        page.scroll.scroll.set_offset(gpui::point(px(0.0), px(-1_000_000.0)));
                    }
                    Err(error) => page.comment_error = Some(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn comment_composer(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let sending = self.comment_task.is_some();
        let can_send = !sending
            && self.detail.is_some()
            && !self.comment_input.read(cx).text().trim().is_empty();
        let mut stack = div().relative().flex().flex_col().gap(px(8.0));
        if self.mention_token.is_some() {
            let popup_theme = theme.for_popup();
            let mut menu = crate::popover::popover_card(&popup_theme)
                .id("pr-mention-menu")
                .debug_selector(|| "pr-mention-menu".into())
                .w_full()
                .max_h(px(240.0))
                .overflow_y_scroll()
                .on_mouse_down_out(cx.listener(|page, _, _, cx| {
                    page.mention_token = None;
                    page.mention_choices.clear();
                    page.sync_mention_controls(cx);
                    cx.notify();
                }));
            if self.mention_choices.is_empty() {
                menu = menu.child(
                    div()
                        .p(px(12.0))
                        .text_size(px(12.0))
                        .text_color(theme.text_muted)
                        .child("No matching PR participants. You can type any @username."),
                );
            }
            for (index, login) in self.mention_choices.iter().enumerate() {
                menu = menu.child(
                    crate::popover::menu_row(
                        &popup_theme,
                        index == self.mention_index,
                        format!("pr-mention-{}-{index}", cx.entity_id()),
                    )
                        .id(SharedString::from(format!("pr-mention-{index}")))
                        .w_full()
                        .child(super::super::pull_request_media::avatar(
                            login,
                            format!("pr-mention-avatar-{index}").into(),
                            20.0,
                            theme,
                        ))
                        .child(format!("@{login}"))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|page, _, window, cx| {
                                window.focus(&page.comment_input.read(cx).focus_handle(cx), cx)
                            }),
                        )
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.mention_index = index;
                            page.accept_person(cx);
                            cx.notify();
                        })),
                );
            }
            stack = stack.child(crate::popover::full_width_menu_above(
                "pr-mention-popup",
                menu.into_any_element(),
                None,
            ));
        }
        let mut surface = div()
            .id("pr-comment-surface")
            .debug_selector(|| "pr-comment-surface".into())
            .p(px(12.0))
            .rounded(px(18.0))
            .border_1()
            .border_color(theme.border)
            .bg(theme.input_glass_bg())
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .max_h(px(120.0))
                    .overflow_hidden()
                    .child(self.comment_input.clone()),
            );
        if let Some(error) = &self.comment_error {
            surface = surface.child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme.danger)
                    .child(error.clone()),
            );
        }
        surface = surface.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.text_muted)
                        .child("Markdown · @ mention"),
                )
                .child(
                    widgets::ghost_action(theme)
                        .id("pr-send-comment")
                        .debug_selector(|| "pr-send-comment".into())
                        .role(gpui::Role::Button)
                        .aria_label("Send PR comment")
                        .tab_index(0)
                        .when(!can_send, |el| el.opacity(0.45))
                        .child(
                            crate::icons::icon(crate::icons::ARROW_UP)
                                .size(px(16.0))
                                .text_color(theme.text),
                        )
                        .child(if sending { "Sending…" } else { "Comment" })
                        .on_click(cx.listener(move |page, _, _, cx| {
                            if can_send {
                                page.send_comment(cx);
                            }
                        })),
                ),
        );
        stack = stack.child(crate::frost::frosted(
            18.0,
            crate::frost::MENU_BLUR,
            surface,
        ));
        div()
            .absolute()
            .bottom(px(72.0))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(div().w_full().max_w(px(760.0)).px(px(40.0)).child(stack))
            .into_any_element()
    }

    pub(super) fn close_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.image_task = None;
        self.image_preview = None;
        self.image_error = None;
        if let Some(focus) = self.image_previous_focus.take() {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    pub(super) fn open_image(&mut self, source: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.image_error = None;
        self.image_previous_focus = window.focused(cx);
        if let Some((cached_source, preview)) = &self.image_cached
            && cached_source == source
        {
            preview.viewer.reset();
            self.image_preview = Some(preview.clone());
            window.focus(&self.image_focus, cx);
            cx.notify();
            return;
        }
        let source = source.to_owned();
        let client = cx.http_client();
        window.focus(&self.image_focus, cx);
        self.image_task = Some(cx.spawn(async move |this, cx| {
            use futures::AsyncReadExt;
            let result: Result<_, String> = async {
                let mut response = client
                    .get(&source, ().into(), true)
                    .await
                    .map_err(|e| e.to_string())?;
                if !response.status().is_success() {
                    return Err(
                        "Image could not be loaded. Use its external link on GitHub.".into(),
                    );
                }
                let mime = response
                    .headers()
                    .get("content-type")
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or("image/png")
                    .to_owned();
                let mut bytes = Vec::new();
                response
                    .body_mut()
                    .take(20 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|e| e.to_string())?;
                cx.background_executor()
                    .spawn(async move { crate::image_media::decode_image(&mime, bytes) })
                    .await
            }
            .await;
            let _ = this.update(cx, |page, cx| {
                page.image_task = None;
                match result {
                    Ok(image) => {
                        let preview =
                            crate::attachments::PreviewImage::new("PR image", image.image);
                        page.image_cached = Some((source, preview.clone()));
                        page.image_preview = Some(preview);
                    }
                    Err(error) => page.image_error = Some(format!("{error} · Click to close")),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AppContext;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CommentRpc(Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl zeron_rpc::RpcService for CommentRpc {
        async fn handle(
            &self,
            method: &str,
            params: serde_json::Value,
        ) -> Result<zeron_rpc::RpcReply, zeron_rpc::RpcError> {
            assert_eq!(method, methods::POST_CHANGE_REQUEST_COMMENT);
            assert_eq!(params["targetDeviceId"], "device");
            let attempt = self.0.fetch_add(1, Ordering::SeqCst);
            if attempt > 0 {
                return Err(zeron_rpc::RpcError::Failed("offline".into()));
            }
            zeron_rpc::RpcReply::value(&zeron_proto::ChangeRequestComment {
                body: params["body"].as_str().unwrap().into(),
                author: zeron_proto::ChangeRequestActor {
                    login: "writer".into(),
                },
                ..Default::default()
            })
        }
    }

    #[gpui::test]
    fn pull_request_image_lightbox_reuses_cached_image_and_closes_with_escape(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::http_client::{FakeHttpClient, Response};
        let raster = image::RgbaImage::from_pixel(200, 100, image::Rgba([120, 160, 210, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        raster.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let bytes = png.into_inner();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let client = FakeHttpClient::create(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            let bytes = bytes.clone();
            async move {
                Ok(Response::builder()
                    .status(200)
                    .header("content-type", "image/png")
                    .body(bytes.into())
                    .unwrap())
            }
        });
        cx.update(|cx| {
            cx.set_global(Theme::default());
            cx.set_http_client(client);
        });
        let (page, cx) = cx.add_window_view(|window, cx| {
            let state = cx.new(|_| AppState::new());
            PullRequestDetailPage::new(
                state,
                "https://github.com/a/b/pull/1".into(),
                None,
                Default::default(),
                None,
                window,
                cx,
            )
        });
        for _ in 0..2 {
            cx.update(|window, cx| {
                page.update(cx, |page, cx| {
                    page.open_image("https://example.com/a.png", window, cx)
                })
            });
            cx.run_until_parked();
            assert!(page.read_with(cx, |page, _| page.image_preview.is_some()));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            assert!(page.read_with(cx, |page, _| page.image_preview.is_none()));
        }
    }

    #[gpui::test]
    fn pull_request_comment_preserves_mentions_drafts_and_prevents_duplicate_submission(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        cx.update(|cx| cx.set_global(Theme::default()));
        let calls = Arc::new(AtomicUsize::new(0));
        let (page, cx) = cx.add_window_view(|window, cx| {
            let state = cx.new(|_| AppState::new());
            let mut page = PullRequestDetailPage::new(
                state.clone(),
                "https://github.com/a/b/pull/1".into(),
                Some("device".into()),
                Default::default(),
                None,
                window,
                cx,
            );
            state.update(cx, |state, _| {
                state.set_test_engine(crate::state::EngineHandle::from_test_client(
                    zeron_rpc::memory_client(Arc::new(CommentRpc(calls.clone()))),
                ))
            });
            page.detail = Some(ChangeRequestDetail {
                author: zeron_proto::ChangeRequestActor {
                    login: "octocat".into(),
                },
                ..Default::default()
            });
            page.body = Some(crate::markdown::parse_full(""));
            page.error = None;
            page.tab = Tab::Activity;
            page
        });
        page.update(cx, |page, cx| {
            page.comment_input
                .update(cx, |input, cx| input.set_text("Hello @oct", cx));
            page.comment_event(&ComposerInputEvent::Edited, cx);
            assert_eq!(page.mention_choices, ["octocat"]);
        });
        cx.run_until_parked();
        let menu = cx.debug_bounds("pr-mention-menu").unwrap();
        let composer = cx.debug_bounds("pr-comment-surface").unwrap();
        assert!(menu.bottom() <= composer.top(), "mentions float above the input");
        assert_eq!(menu.left(), composer.left());
        assert_eq!(menu.right(), composer.right());
        page.update(cx, |page, cx| {
            page.comment_event(&ComposerInputEvent::MentionAccept, cx);
            assert_eq!(page.comment_input.read(cx).text(), "Hello @octocat ");
            page.send_comment(cx);
            page.send_comment(cx);
        });
        for _ in 0..100 {
            cx.run_until_parked();
            runtime.block_on(async { tokio::task::yield_now().await });
            if page.read_with(cx, |page, _| page.comment_task.is_none()) {
                break;
            }
        }
        page.update(cx, |page, cx| {
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert!(page.comment_input.read(cx).text().is_empty());
            assert_eq!(
                page.detail.as_ref().unwrap().comments[0].body,
                "Hello @octocat "
            );
            assert_eq!(page.activity_bodies.len(), 1);
            page.comment_input
                .update(cx, |input, cx| input.set_text("Keep this draft", cx));
            page.send_comment(cx);
        });
        for _ in 0..100 {
            cx.run_until_parked();
            runtime.block_on(async { tokio::task::yield_now().await });
            if page.read_with(cx, |page, _| page.comment_task.is_none()) {
                break;
            }
        }
        page.read_with(cx, |page, cx| {
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert_eq!(page.comment_input.read(cx).text(), "Keep this draft");
            assert!(page.comment_error.is_some());
            assert_eq!(page.detail.as_ref().unwrap().comments.len(), 1);
        });
    }
}
