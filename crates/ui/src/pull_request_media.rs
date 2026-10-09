//! GitHub description media is opt-in; transcript Markdown keeps its existing policy.
use std::{cell::RefCell, rc::Rc};

use gpui::{AnyElement, IntoElement, SharedString, div, prelude::*, px};
use html5ever::tokenizer::{BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer};
use pulldown_cmark::{Event, Options, Parser};

use crate::{
    markdown::{self, BlockTree, parser::InlineImage, render::MediaUi},
    theme::Theme,
};

#[derive(Default)]
struct DescriptionHtml {
    text: RefCell<String>,
    hidden: RefCell<Option<String>>,
}

impl TokenSink for DescriptionHtml {
    type Handle = ();

    fn process_token(&self, token: Token, _: u64) -> TokenSinkResult<()> {
        let mut out = self.text.borrow_mut();
        match token {
            Token::TagToken(tag) => {
                let name = tag.name.as_ref();
                if let Some(hidden) = self.hidden.borrow().as_ref() {
                    if tag.kind != TagKind::EndTag || name != hidden {
                        return TokenSinkResult::Continue;
                    }
                }
                if matches!(name, "script" | "style" | "iframe") {
                    *self.hidden.borrow_mut() =
                        (tag.kind == TagKind::StartTag).then(|| name.to_owned());
                    return TokenSinkResult::Continue;
                }
                if name == "img" && tag.kind == TagKind::StartTag {
                    let attr = |key: &str| {
                        tag.attrs
                            .iter()
                            .find(|a| a.name.local.as_ref() == key)
                            .map(|a| a.value.as_ref())
                            .unwrap_or("")
                    };
                    let src = attr("src").replace('<', "%3C").replace('>', "%3E");
                    let alt = attr("alt")
                        .replace('\\', "\\\\")
                        .replace('[', "\\[")
                        .replace(']', "\\]")
                        .replace(['\r', '\n'], " ");
                    if !src.is_empty() {
                        out.push_str(&format!("![{alt}](<{src}>)"));
                    }
                } else if matches!(name, "details" | "summary" | "p" | "div" | "br") {
                    out.push_str("\n\n");
                }
            }
            Token::CharacterTokens(text) if self.hidden.borrow().is_none() => out.push_str(&text),
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

/// Convert HTML tokens only: tags in inline code and fenced examples remain literal.
/// No DOM, scripts, styles, or event-handler attributes are executed.
fn description_markdown(source: &str) -> String {
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for (event, range) in Parser::new_ext(source, Options::all()).into_offset_iter() {
        if matches!(event, Event::Html(_) | Event::InlineHtml(_)) {
            if let Some(last) = ranges.last_mut().filter(|last| last.end == range.start) {
                last.end = range.end;
            } else {
                ranges.push(range);
            }
        }
    }
    let mut result = String::with_capacity(source.len());
    let mut end = 0;
    for range in ranges {
        result.push_str(&source[end..range.start]);
        let input = BufferQueue::default();
        input.push_back(source[range.clone()].into());
        let tokenizer = Tokenizer::new(DescriptionHtml::default(), Default::default());
        let _ = tokenizer.feed(&input);
        tokenizer.end();
        let converted = tokenizer.sink.text.borrow();
        result.push_str(&converted);
        // An image renders as its own block, so the line break after it
        // would otherwise open the next line with a space.
        if converted.ends_with(">)") && source[range.end..].starts_with(['\r', '\n']) {
            result.push_str("\n\n");
        }
        end = range.end;
    }
    result.push_str(&source[end..]);
    result
}

pub(super) fn parse_description(source: &str) -> BlockTree {
    markdown::parse_full(&description_markdown(source))
}

fn image_url(source: &str, pr_url: &str) -> Option<url::Url> {
    let mut url = if source.starts_with('/') {
        url::Url::parse(pr_url).ok()?.join(source).ok()?
    } else {
        url::Url::parse(source).ok()?
    };
    if !crate::asset_http::public_https(&url) {
        return None;
    }
    // GitHub's file viewer serves HTML; load its raw image instead.
    if url.host_str() == Some("github.com") {
        let path = url.path().to_owned();
        let parts: Vec<_> = path.split('/').collect();
        if parts.len() > 5 && parts[3] == "blob" {
            url.set_path(&format!(
                "/{}/{}/{}",
                parts[1],
                parts[2],
                parts[4..].join("/")
            ));
            url.set_host(Some("raw.githubusercontent.com")).ok()?;
        }
    }
    Some(url)
}

/// GitHub serves the image itself. Anything else is another party's server,
/// which would learn who opened the pull request and when.
fn github_hosted(url: &url::Url) -> bool {
    url.host_str().is_some_and(|host| {
        host == "github.com"
            || host.ends_with(".githubusercontent.com")
            || host.ends_with(".githubassets.com")
    })
}

fn placeholder(label: String, theme: &Theme) -> AnyElement {
    div()
        .w_full()
        .min_h(px(80.0))
        .p(px(16.0))
        .rounded(px(8.0))
        .bg(theme.glass_hover())
        .text_size(px(12.0))
        .text_color(theme.text_muted)
        .child(label)
        .into_any_element()
}

/// Images in pull request text, which anyone can write: fetched through the
/// app's guarded HTTP client (HTTPS to public addresses, 16 MiB) and decoded
/// with the app's bounded decoder — 4096 px per side, 64 MiB of decoder
/// memory, one frame of an animation, SVG with external references removed.
/// GPUI's own image loader decodes without limits, so a few-KB image could
/// otherwise exhaust memory. At most four load at once.
struct PrImageLoader;

/// Largest image body read; the transport enforces the same cap.
const PR_IMAGE_MAX_BYTES: u64 = 16 * 1024 * 1024;

fn pr_image_slots() -> &'static tokio::sync::Semaphore {
    static SLOTS: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| tokio::sync::Semaphore::new(4))
}

/// SVG by declared type or by content; everything else decodes as raster.
fn pr_image_mime(content_type: Option<&str>, bytes: &[u8]) -> &'static str {
    let declared_svg = content_type.is_some_and(|value| value.starts_with("image/svg+xml"));
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
    if declared_svg || (image::guess_format(bytes).is_err() && head.contains("<svg")) {
        "image/svg+xml"
    } else {
        "image/png"
    }
}

impl gpui::Asset for PrImageLoader {
    type Source = SharedString;
    type Output = Result<crate::image_media::MediaImage, SharedString>;

    fn load(
        source: Self::Source,
        cx: &mut gpui::App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        let client = cx.http_client();
        async move {
            use futures::AsyncReadExt as _;
            let _slot = pr_image_slots()
                .acquire()
                .await
                .map_err(|_| SharedString::from("image loading stopped"))?;
            let mut response = client
                .get(source.as_ref(), ().into(), true)
                .await
                .map_err(|error| SharedString::from(error.to_string()))?;
            if !response.status().is_success() {
                return Err(format!("image request failed: {}", response.status()).into());
            }
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let mut bytes = Vec::new();
            response
                .body_mut()
                .take(PR_IMAGE_MAX_BYTES + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|error| SharedString::from(error.to_string()))?;
            if bytes.len() as u64 > PR_IMAGE_MAX_BYTES {
                return Err("image is too large to preview".into());
            }
            let mime = pr_image_mime(content_type.as_deref(), &bytes);
            crate::image_media::decode_image(mime, bytes).map_err(SharedString::from)
        }
    }
}

/// GPUI's Img resolves percentage width + auto height to the source pixel height.
/// Measure both dimensions together, then give Img explicit pixel dimensions.
struct PreviewImage {
    source: SharedString,
    id: SharedString,
    alt: String,
    theme: Theme,
}

impl IntoElement for PreviewImage {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for PreviewImage {
    type RequestLayoutState = Option<Result<crate::image_media::MediaImage, SharedString>>;
    type PrepaintState = AnyElement;

    fn id(&self) -> Option<gpui::ElementId> {
        Some(self.id.clone().into())
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        let data = window.use_asset::<PrImageLoader>(&self.source, cx);
        let natural = data
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .map(|image| gpui::size(px(image.width), px(image.height)));
        let layout =
            window.request_measured_layout(Default::default(), move |known, available, _, _| {
                let width = known
                    .width
                    .or(match available.width {
                        gpui::AvailableSpace::Definite(width) => Some(width),
                        _ => None,
                    })
                    .unwrap_or(px(320.0));
                if let Some(natural) = natural {
                    let scale = (f32::from(width) / f32::from(natural.width).max(1.0))
                        .min(420.0 / f32::from(natural.height).max(1.0))
                        .min(1.0);
                    gpui::size(natural.width * scale, natural.height * scale)
                } else {
                    gpui::size(width, px(80.0))
                }
            });
        (layout, data)
    }

    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        data: &mut Self::RequestLayoutState,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> AnyElement {
        let mut child = match data {
            Some(Ok(image)) => gpui::img(image.image.clone())
                .id(self.id.clone())
                .debug_selector(|| "pr-description-image".into())
                .w(bounds.size.width)
                .h(bounds.size.height)
                .rounded(px(8.0))
                .border_1()
                .border_color(crate::theme::hairline(0.1))
                .object_fit(gpui::ObjectFit::Contain)
                .into_any_element(),
            Some(Err(_)) => div()
                .debug_selector(|| "pr-description-image-error".into())
                .w(bounds.size.width)
                .child(placeholder(
                    format!("{} · Couldn’t load image. Open in browser ↗", self.alt),
                    &self.theme,
                ))
                .into_any_element(),
            None => placeholder("Loading image…".into(), &self.theme),
        };
        child.prepaint_as_root(
            bounds.origin,
            bounds.size.map(gpui::AvailableSpace::Definite),
            window,
            cx,
        );
        child
    }

    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<gpui::Pixels>,
        _: &mut Self::RequestLayoutState,
        child: &mut AnyElement,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) {
        child.paint(window, cx);
    }
}

pub(super) fn media(
    pr_url: &str,
    open: Rc<dyn Fn(&str, &mut gpui::Window, &mut gpui::App)>,
) -> MediaUi {
    let pr_url = pr_url.to_owned();
    MediaUi {
        diagram: None,
        image: image_renderer(move |image, id, theme| {
            let Some(source) = image_url(&image.source, &pr_url) else {
                return placeholder(
                    "Image unavailable here. Open the pull request in your browser to see it."
                        .into(),
                    theme,
                );
            };
            let alt = if image.alt.is_empty() {
                "Image".to_owned()
            } else {
                image.alt.clone()
            };
            let inline = github_hosted(&source);
            let host = source.host_str().unwrap_or_default().to_owned();
            let source = String::from(source);
            let tile_selector = format!("pr-image-tile-{id}");
            let tile = div()
                .id(id.clone())
                .debug_selector(move || tile_selector.clone())
                .w_full()
                .min_w_0()
                .my(px(8.0))
                .rounded(px(8.0))
                .flex()
                .justify_center()
                .overflow_hidden()
                .role(gpui::Role::Button)
                .tab_index(0)
                .focus_visible(|style| style.border_2().border_color(theme.accent))
                .cursor_pointer()
                .on_click({
                    let source = source.clone();
                    let open = open.clone();
                    move |_, window, cx| {
                        cx.stop_propagation();
                        // A failed inline image offers the browser instead.
                        if matches!(
                            window.get_asset::<PrImageLoader>(&SharedString::from(source.clone()), cx),
                            Some(Err(_))
                        ) {
                            cx.open_url(&source);
                        } else {
                            open(&source, window, cx);
                        }
                    }
                });
            if inline {
                tile.aria_label(format!("Open image: {alt}"))
                    .child(PreviewImage {
                        source: source.into(),
                        id: format!("{id}-image").into(),
                        alt,
                        theme: theme.clone(),
                    })
                    .into_any_element()
            } else {
                // Loaded only when asked for.
                tile.aria_label(format!("Load image from {host}: {alt}"))
                    .child(placeholder(
                        format!("{alt} · hosted on {host} · Click to load"),
                        theme,
                    ))
                    .into_any_element()
            }
        }),
    }
}

/// Types the image closure for [`MediaUi::image`].
fn image_renderer(
    render: impl Fn(&InlineImage, SharedString, &Theme) -> AnyElement + 'static,
) -> Option<Rc<dyn Fn(&InlineImage, SharedString, &Theme) -> AnyElement>> {
    Some(Rc::new(render))
}

/// Screenshot tables become wrapping media cards instead of tall table cells.
/// Ordinary data tables retain their Markdown layout and reading order.
pub(super) fn render_description(
    body: &BlockTree,
    options: &markdown::render::RenderOptions,
    theme: &Theme,
    window: &mut gpui::Window,
) -> AnyElement {
    use markdown::parser::{Block, InlineRun};
    let image_ui = options.media.as_ref().and_then(|media| media.image.clone());
    let mut output = div().w_full().min_w_0().flex().flex_col().gap(px(8.0));
    for (block_index, block) in body.blocks.iter().enumerate() {
        let cells: Option<Vec<Vec<InlineRun>>> = match &block.block {
            Block::Table { header, rows, .. }
                if rows
                    .iter()
                    .flatten()
                    .flatten()
                    .any(|run| run.style.image.is_some()) =>
            {
                Some(
                    rows.iter()
                        .flat_map(|row| {
                            row.iter().enumerate().map(|(column, cell)| {
                                let mut runs = header.get(column).cloned().unwrap_or_default();
                                runs.extend(cell.clone());
                                runs
                            })
                        })
                        .collect(),
                )
            }
            Block::Paragraph { runs }
                if runs.iter().any(|run| run.style.image.is_some())
                    && runs
                        .iter()
                        .all(|run| run.style.image.is_some() || run.text.trim().is_empty()) =>
            {
                Some(
                    runs.iter()
                        .filter(|run| run.style.image.is_some())
                        .cloned()
                        .map(|run| vec![run])
                        .collect(),
                )
            }
            _ => None,
        };
        if let (Some(cells), Some(image_ui)) = (cells, &image_ui) {
            let mut grid = div().w_full().min_w_0().flex().flex_wrap().gap(px(12.0));
            for (cell_index, runs) in cells.into_iter().enumerate() {
                let mut card = div()
                    .flex_basis(px(240.0))
                    .flex_grow(1.0)
                    .min_w_0()
                    .max_w_full();
                let label = runs
                    .iter()
                    .filter(|run| run.style.image.is_none())
                    .map(|run| run.text.as_str())
                    .collect::<String>();
                if !label.trim().is_empty() {
                    card = card.child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme.text_muted)
                            .child(label),
                    );
                }
                for (index, run) in runs.iter().enumerate() {
                    if let Some(image) = &run.style.image {
                        let id = format!(
                            "{}-gallery-{block_index}-{cell_index}-{index}",
                            options.row_key
                        );
                        card = card.child(image_ui(image, id.into(), theme));
                    }
                }
                grid = grid.child(card);
            }
            output = output.child(grid);
        } else {
            let mut scoped = options.clone();
            scoped.row_key = format!("{}-block-{block_index}", options.row_key).into();
            output = output.child(markdown::render::render_tree(
                &BlockTree {
                    blocks: vec![block.clone()],
                },
                &scoped,
                theme,
                window,
                &|_| None,
            ));
        }
    }
    output.into_any_element()
}

/// GitHub redirects this public profile image endpoint to its avatar CDN.
/// Stable sizing lets repeated authors share GPUI's image cache.
pub(super) fn avatar(login: &str, id: SharedString, size: f32, theme: &Theme) -> AnyElement {
    let initial = login
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into());
    let fallback_theme = theme.clone();
    let fallback = move || {
        div()
            .size(px(size))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(fallback_theme.glass_hover())
            .text_color(fallback_theme.text_muted)
            .text_size(px(size * 0.5))
            .child(initial.clone())
            .into_any_element()
    };
    if login.is_empty() {
        return fallback();
    }
    let mut url = url::Url::parse("https://github.com/").unwrap();
    url.path_segments_mut()
        .unwrap()
        .push(&format!("{login}.png"));
    url.set_query(Some("size=64"));
    div()
        .id(id.clone())
        .size(px(size))
        .flex_none()
        .rounded_full()
        .overflow_hidden()
        .child(
            gpui::img(SharedString::from(url.to_string()))
                .id(id)
                .size(px(size))
                .rounded_full()
                .object_fit(gpui::ObjectFit::Cover)
                .with_loading(fallback.clone())
                .with_fallback(fallback),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request_test_support as fixture;

    #[test]
    fn pull_request_html_images_handle_multiline_attributes_and_entities() {
        let text = "Before\n\n<img width=900\nalt='A &amp; B' src='https://example.com/a.png?x=1&amp;y=2'/>\n\nAfter";
        let converted = description_markdown(text);
        assert!(
            converted.contains("![A & B](<https://example.com/a.png?x=1&y=2>)"),
            "{converted}"
        );
        let tree = parse_description(text);
        assert!(tree.blocks.iter().any(|b| matches!(&b.block, markdown::parser::Block::Paragraph { runs } if runs.iter().any(|r| r.style.image.is_some()))));
    }

    #[test]
    fn pull_request_html_preserves_code_and_description_content() {
        let code = "`<img src='code.png'>`\n\n```html\n<img src='example.png'>\n```";
        assert_eq!(description_markdown(code), code);
        let converted = description_markdown(
            "<details>\n<summary>Screenshots</summary>\n\n![shot](https://example.com/a.png)\n\n</details>",
        );
        assert!(converted.contains("Screenshots"));
        assert!(converted.contains("![shot]"));
        assert!(!converted.contains("<details>"));
        assert!(!description_markdown("<script><img src=x></script>").contains("!["));
        // Text on the line after an image starts a paragraph, not " text".
        let tree =
            parse_description("Look\r\n<img src=\"https://example.com/a.png\" />\r\nthen this");
        let last = tree.blocks.last().map(|block| &block.block);
        assert!(
            matches!(last, Some(markdown::parser::Block::Paragraph { runs }) if runs[0].text == "then this"),
            "{:?}",
            tree.blocks
        );
    }

    #[test]
    fn pull_request_image_urls_only_load_web_media_and_normalize_github_blobs() {
        let pr = "https://github.com/owner/repo/pull/1";
        assert_eq!(
            image_url("/user-attachments/assets/abc", pr)
                .unwrap()
                .as_str(),
            "https://github.com/user-attachments/assets/abc"
        );
        assert_eq!(
            image_url("https://github.com/o/r/blob/main/image.png", pr)
                .unwrap()
                .as_str(),
            "https://raw.githubusercontent.com/o/r/main/image.png"
        );
        for source in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:image/svg+xml,test",
            "https://name:pass@example.com/a",
            "http://example.com/a.png",
            "https://192.168.1.10/a.png",
            "relative.png",
        ] {
            assert!(image_url(source, pr).is_none(), "{source}");
        }
    }
    struct MediaFixture {
        description: String,
    }

    impl gpui::Render for MediaFixture {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let options = markdown::render::RenderOptions {
                media: Some(media(
                    "https://github.com/a/b/pull/1",
                    Rc::new(|_, _, _| {}),
                )),
                tasks: None,
                row_key: "media-test".into(),
                veil: None,
                cache: None,
                now: std::time::Instant::now(),
                copy: None,
                link: None,
                workspace_root: None,
                code: None,
            };
            div().size_full().p(px(24.0)).child(render_description(
                &parse_description(&self.description),
                &options,
                Theme::of(cx),
                window,
            ))
        }
    }

    #[gpui::test]
    fn pull_request_description_fetches_images_and_shows_load_failures(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::http_client::{FakeHttpClient, Response};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let raster = image::RgbaImage::from_pixel(2400, 1200, image::Rgba([120, 160, 210, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        raster.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let bytes = png.into_inner();
        let portrait = image::RgbaImage::from_pixel(1200, 2600, image::Rgba([120, 160, 210, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        portrait
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let portrait_bytes = png.into_inner();
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let client = FakeHttpClient::create(move |request| {
            seen.fetch_add(1, Ordering::SeqCst);
            let bytes = if request.uri().path() == "/portrait.png" {
                portrait_bytes.clone()
            } else {
                bytes.clone()
            };
            async move {
                if request.uri().path() == "/missing.png" {
                    Ok(Response::builder()
                        .status(404)
                        .body(Default::default())
                        .unwrap())
                } else {
                    Ok(Response::builder()
                        .status(200)
                        .header("content-type", "image/png")
                        .body(bytes.into())
                        .unwrap())
                }
            }
        });
        fixture::init(cx);
        cx.update(|cx| {
            cx.set_http_client(client);
        });
        let (view, cx) = cx.add_window_view(|_, _| MediaFixture {
            description: "<img src='https://private-user-images.githubusercontent.com/image.png' alt='Screenshot'/>".into(),
        });
        cx.simulate_resize(gpui::size(px(400.0), px(600.0)));
        cx.run_until_parked();
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        let bounds = cx
            .debug_bounds("pr-description-image")
            .expect("rendered image");
        assert!(
            (f32::from(bounds.size.width) / f32::from(bounds.size.height) - 2.0).abs() < 0.05,
            "image must retain its natural aspect ratio: {bounds:?}"
        );
        assert!(
            bounds.size.height <= px(420.0),
            "preview height: {bounds:?}"
        );
        assert!(
            bounds.left() >= px(24.0) && bounds.right() <= px(376.0),
            "{bounds:?}"
        );
        assert!(cx.debug_bounds("pr-description-image-error").is_none());
        view.update(cx, |view, cx| {
            view.description = "| Before | After |\n| --- | --- |\n| ![a](https://private-user-images.githubusercontent.com/image.png) | ![b](https://private-user-images.githubusercontent.com/image.png) |".into();
            cx.notify();
        });
        for width in [800.0, 320.0] {
            cx.simulate_resize(gpui::size(px(width), px(900.0)));
            cx.run_until_parked();
            let first = cx
                .debug_bounds("pr-image-tile-media-test-gallery-0-0-1")
                .unwrap();
            let second = cx
                .debug_bounds("pr-image-tile-media-test-gallery-0-1-1")
                .unwrap();
            assert!(
                first.size.height < first.size.width,
                "thumbnail must be landscape: {first:?}"
            );
            assert!(first.right() <= px(width - 24.0));
            if width > 600.0 {
                assert_eq!(first.top(), second.top());
            } else {
                assert!(second.top() >= first.bottom());
            }
        }
        view.update(cx, |view, cx| {
            view.description =
                "![portrait](https://private-user-images.githubusercontent.com/portrait.png)"
                    .into();
            cx.notify();
        });
        for width in [320.0, 1200.0] {
            cx.simulate_resize(gpui::size(px(width), px(900.0)));
            cx.run_until_parked();
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
            let image = cx.debug_bounds("pr-description-image").unwrap();
            let tile = cx
                .debug_bounds("pr-image-tile-media-test-gallery-0-0-0")
                .unwrap();
            assert!(image.size.height <= px(420.0), "{image:?}");
            assert!(
                (f32::from(image.size.width) / f32::from(image.size.height) - 1200.0 / 2600.0)
                    .abs()
                    < 0.01,
                "{image:?}"
            );
            assert_eq!(
                tile.size.height, image.size.height,
                "tile must not reserve source pixel height"
            );
            assert!(image.left() >= px(24.0) && image.right() <= px(width - 24.0));
        }
        view.update(cx, |view, cx| {
            view.description =
                "![missing](https://private-user-images.githubusercontent.com/missing.png)".into();
            cx.notify();
        });
        cx.run_until_parked();
        // Asset completion invalidates the host on the next UI frame.
        cx.update(|window, _| window.refresh());
        cx.cx.run_until_parked();
        assert_eq!(requests.load(Ordering::SeqCst), 3);
        let error = cx.debug_bounds("pr-description-image-error");
        let image = cx.debug_bounds("pr-description-image");
        assert!(error.is_some(), "image bounds: {image:?}");
        // Another party's server is contacted only on request.
        view.update(cx, |view, cx| {
            view.description = "![chart](https://example.com/chart.png)".into();
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert_eq!(requests.load(Ordering::SeqCst), 3, "nothing is fetched");
        assert!(cx.debug_bounds("pr-description-image").is_none());
        assert!(
            cx.debug_bounds("pr-image-tile-media-test-gallery-0-0-0")
                .is_some()
        );
    }
}
