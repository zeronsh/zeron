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
                if matches!(event, ComposerInputEvent::Edited) {
                    self.save_draft(cx);
                }
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
        if self.sending || self.detail.is_none() || body.trim().is_empty() {
            return;
        }
        if body.len() > 60_000 {
            self.comment_error = Some("Comment is too long (maximum 60,000 bytes).".into());
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.comment_error =
                Some("Connect to the pull request’s device to send your comment.".into());
            return;
        };
        self.task = None;
        self.loading = false;
        self.comment_error = None;
        self.sending = true;
        let mut params = self.params(false);
        params["body"] = body.clone().into();
        let (cache, target, url) = (self.cache.clone(), self.target.clone(), self.url.clone());
        // Detached: leaving the pull request neither stops the post nor loses
        // its outcome.
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::POST_CHANGE_REQUEST_COMMENT, params)
                .await
                .map_err(|error| {
                    format!(
                        "Couldn’t confirm your comment was posted. {} Your draft is kept; check GitHub before sending again.",
                        super::failure_reason(&error)
                    )
                })
                .and_then(|value| {
                    serde_json::from_value::<zeron_proto::ChangeRequestComment>(value).map_err(
                        |_| {
                            "Your comment was posted, but its reply couldn’t be read. Refresh to see it."
                                .to_owned()
                        },
                    )
                });
            let result = cx
                .background_executor()
                .spawn(async move {
                    result.map(|comment| {
                        let parsed =
                            super::super::pull_request_media::parse_description(&comment.body);
                        (comment, parsed)
                    })
                })
                .await;
            let failure = result.as_ref().err().cloned();
            let shown = this.update(cx, |page, cx| {
                page.sending = false;
                match result {
                    Ok((comment, parsed)) => {
                        if let Some(detail) = &mut page.detail {
                            page.activity_bodies.insert(detail.comments.len(), parsed);
                            detail.comments.push(comment);
                        }
                        if page.comment_input.read(cx).text() == body {
                            page.comment_input
                                .update(cx, |input, cx| input.set_text("", cx));
                        }
                        page.save_draft(cx);
                        if let (Some(detail), Some(parsed)) = (&page.detail, &page.body) {
                            let snapshot = DetailSnapshot {
                                detail: detail.clone(),
                                body: parsed.clone(),
                                activity: page.activity_bodies.clone(),
                                fetched: page.fetched.unwrap_or_else(Instant::now),
                                diff: page.diff_snapshot(),
                            };
                            page.cache.borrow_mut().put(
                                page.target.clone(),
                                page.url.clone(),
                                snapshot,
                            );
                        }
                        page.scroll
                            .scroll
                            .set_offset(gpui::point(px(0.0), px(-1_000_000.0)));
                    }
                    Err(error) => {
                        page.comment_error = Some(error);
                        page.save_draft(cx);
                    }
                }
                cx.notify();
            });
            if shown.is_err() {
                // The view is gone. A posted comment makes the cached thread
                // stale; a failed one waits as a draft with the reason.
                let mut cache = cache.borrow_mut();
                match failure {
                    None => {
                        cache.set_draft(&target, &url, "", None);
                        cache.evict(&target, &url);
                    }
                    Some(error) => cache.set_draft(&target, &url, &body, Some(error)),
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Keep the unsent comment for the next visit to this pull request.
    fn save_draft(&self, cx: &App) {
        self.cache.borrow_mut().set_draft(
            &self.target,
            &self.url,
            self.comment_input.read(cx).text(),
            self.comment_error.clone(),
        );
    }

    pub(super) fn comment_composer(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let sending = self.sending;
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
                        .child("No matching participants. You can type any @username."),
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
        // The chat composer's compact pill: same surface, radius, height and
        // insets, with the send circle on the last line's centerline.
        let row_height = crate::composer::COMPACT_TOTAL_HEIGHT - crate::composer::PILL_BORDER_V;
        let mut surface = div()
            .id("pr-comment-surface")
            .debug_selector(|| "pr-comment-surface".into())
            .min_h(px(crate::composer::COMPACT_TOTAL_HEIGHT))
            .rounded(px(crate::composer::COMPOSER_RADIUS))
            .border_1()
            .border_color(theme.composer_surface_border())
            .bg(theme.composer_surface_bg())
            .when(!theme.is_frost(), |el| el.shadow_lg())
            .flex()
            .flex_col()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|page, _, window, cx| {
                    window.focus(&page.comment_input.read(cx).focus_handle(cx), cx)
                }),
            );
        if let Some(error) = &self.comment_error {
            surface = surface.child(
                div()
                    .id("pr-comment-error")
                    .debug_selector(|| "pr-comment-error".into())
                    .px(px(16.0))
                    .pt(px(12.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.danger)
                    .child(error.clone()),
            );
        }
        surface = surface.child(
            div()
                .flex()
                .items_end()
                .pl(px(16.0))
                .pr(px(8.0))
                .gap(px(crate::composer::ACTION_PRIMARY_GAP))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h(px(row_height))
                        .max_h(px(140.0))
                        .py(px(12.0))
                        .flex()
                        .items_center()
                        .overflow_hidden()
                        .child(div().w_full().child(self.comment_input.clone())),
                )
                .child(
                    div()
                        .h(px(row_height))
                        .flex_none()
                        .flex()
                        .items_center()
                        .child(
                            // While `gh` posts, a spinner stands in for the arrow.
                            if sending {
                                div()
                                    .id("pr-send-comment")
                                    .size(px(28.0))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(crate::loaders::mini_glyph_spinner(
                                        "pr-send-spinner",
                                        1.5,
                                        theme.glyph,
                                        cx.entity_id(),
                                        cx,
                                    ))
                            } else {
                                crate::composer::send_circle("pr-send-comment", !can_send, theme)
                            }
                            .debug_selector(|| "pr-send-comment".into())
                            .role(gpui::Role::Button)
                            .aria_label(if sending {
                                "Sending comment"
                            } else {
                                "Send comment"
                            })
                            .tab_index(0)
                            .focus_visible(|style| style.border_2().border_color(theme.accent))
                            .on_click(cx.listener(
                                move |page, _, _, cx| {
                                    cx.stop_propagation();
                                    if can_send {
                                        page.send_comment(cx);
                                    }
                                },
                            )),
                        ),
                ),
        );
        stack = stack.child(crate::frost::frosted(
            crate::composer::COMPOSER_RADIUS,
            16.0,
            surface,
        ));
        // Docked in the flow between the thread and the section navigation:
        // the thread ends above it instead of scrolling behind two stacked
        // floating layers. Same column as the thread, so their edges align.
        div()
            .id("pr-comment-dock")
            .debug_selector(|| "pr-comment-dock".into())
            .flex_none()
            .w_full()
            .pb(px(NAV_CLEARANCE))
            .child(widgets::page_column().pt_0().pb_0().child(stack))
            .into_any_element()
    }

    pub(super) fn close_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.image_task = None;
        self.image_preview = None;
        self.image_failed = None;
        if let Some(focus) = self.image_previous_focus.take() {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    pub(super) fn open_image(&mut self, source: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.image_failed = None;
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
            let result: Result<_, ()> = async {
                let mut response = client.get(&source, ().into(), true).await.map_err(|_| ())?;
                if !response.status().is_success() {
                    return Err(());
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
                    .map_err(|_| ())?;
                cx.background_executor()
                    .spawn(async move { crate::image_media::decode_image(&mime, bytes) })
                    .await
                    .map_err(|_| ())
            }
            .await;
            let _ = this.update(cx, |page, cx| {
                page.image_task = None;
                match result {
                    Ok(image) => {
                        let preview = crate::attachments::PreviewImage::new(
                            "Pull request image",
                            image.image,
                        );
                        page.image_cached = Some((source, preview.clone()));
                        page.image_preview = Some(preview);
                    }
                    Err(()) => page.image_failed = Some(source),
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
        assert!(
            menu.bottom() <= composer.top(),
            "mentions float above the input"
        );
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
            if page.read_with(cx, |page, _| !page.sending) {
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
            if page.read_with(cx, |page, _| !page.sending) {
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

    #[gpui::test]
    fn pull_request_comment_outlives_the_view_and_a_failed_one_returns_as_a_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        cx.update(|cx| cx.set_global(Theme::default()));
        let calls = Arc::new(AtomicUsize::new(0));
        let url = "https://github.com/a/b/pull/1";
        let target = Some("device".to_owned());
        let cache = Rc::new(RefCell::new(PullRequestCache::default()));
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.set_test_engine(crate::state::EngineHandle::from_test_client(
                zeron_rpc::memory_client(Arc::new(CommentRpc(calls.clone()))),
            ));
            state
        });
        let open = |cx: &mut gpui::TestAppContext| {
            cache.borrow_mut().put(
                target.clone(),
                url.into(),
                DetailSnapshot {
                    detail: ChangeRequestDetail::default(),
                    body: crate::markdown::parse_full(""),
                    activity: Vec::new(),
                    fetched: Instant::now(),
                    diff: None,
                },
            );
            cx.add_window(|window, cx| {
                PullRequestDetailPage::new(
                    state.clone(),
                    url.into(),
                    target.clone(),
                    cache.clone(),
                    None,
                    window,
                    cx,
                )
            })
        };
        // Send, then leave before GitHub answers.
        let send_and_leave = |cx: &mut gpui::TestAppContext, text: &str, expected_calls: usize| {
            let window = open(cx);
            window
                .update(cx, |page, window, cx| {
                    page.comment_input
                        .update(cx, |input, cx| input.set_text(text, cx));
                    page.send_comment(cx);
                    window.remove_window();
                })
                .unwrap();
            for _ in 0..100 {
                cx.run_until_parked();
                runtime.block_on(async { tokio::task::yield_now().await });
            }
            assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
        };

        send_and_leave(cx, "Posted after leaving", 1);
        assert!(
            cache.borrow_mut().get(&target, url).is_none(),
            "the cached thread predates the comment"
        );
        assert!(cache.borrow().drafts.is_empty());

        send_and_leave(cx, "Keep me", 2);
        let window = open(cx);
        window
            .update(cx, |page, _, cx| {
                assert_eq!(page.comment_input.read(cx).text(), "Keep me");
                assert!(page.comment_error.is_some());
            })
            .unwrap();
    }
}
