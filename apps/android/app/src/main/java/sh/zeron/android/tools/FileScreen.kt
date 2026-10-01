package sh.zeron.android.tools

import sh.zeron.android.feedback.tapAction
import sh.zeron.android.feedback.feedbackAction
import sh.zeron.android.feedback.OpenCloseFeedback
import sh.zeron.android.feedback.LocalFeedback
import sh.zeron.android.feedback.Haptic
import sh.zeron.android.feedback.Cue
import android.annotation.SuppressLint
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.OffsetMapping
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.input.TransformedText
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.userMessage
import sh.zeron.android.design.GeistMono
import sh.zeron.android.design.LocalDarkTheme
import sh.zeron.android.design.TranscriptPalette
import sh.zeron.android.design.ZIcons
import sh.zeron.android.ui.ActionMenu
import sh.zeron.android.ui.MenuAction
import sh.zeron.android.ui.StatusBanner
import uniffi.zeron_core.HighlightedSource
import uniffi.zeron_core.HostStream
import uniffi.zeron_core.highlightSource
import uniffi.zeron_core.markdownHtml

/**
 * One workspace file: code with the desktop editor's highlighting, line
 * numbers and horizontal scroll, edited in place and saved with the engine's
 * conflict check; Markdown previewed, HTML opened in the browser, images and
 * PDFs shown natively, anything else saved or handed to another app.
 */
@Composable
fun FileScreen(model: AppModel, ref: WorkspaceRef, path: String, onBack: () -> Unit, onBrowser: (String) -> Unit) {
    val kind = remember(path) { FileKind.of(path) }
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val clipboard = LocalClipboardManager.current
    val name = path.substringAfterLast('/')
    val parent = path.substringBeforeLast('/', "")
    val subtitle = listOfNotNull(parent.ifEmpty { ref.title }, ref.deviceName).joinToString(" · ")
    var overflow by remember { mutableStateOf(false) }

    val common = listOfNotNull(
        MenuAction("Save to Downloads", ZIcons.Save) { model.downloads.saveFile(ref, path) },
        MenuAction("Open with…", ZIcons.Link) { scope.launch { runCatching { openWith(context, model, ref, path) }.onFailure { toast(context, it.userMessage()) } } },
        if (kind == FileKind.Html || kind == FileKind.Svg) MenuAction("Open in browser", ZIcons.Globe) { onBrowser(Browser.workspaceUrl(ref, path)) } else null,
        MenuAction("Copy path", ZIcons.Copy, haptic = sh.zeron.android.feedback.Haptic.Confirm, cue = sh.zeron.android.feedback.Cue.Copy) { clipboard.setText(AnnotatedString(ref.absolute(path) ?: path)) },
    )

    when (kind) {
        FileKind.Pdf, FileKind.Image, FileKind.Svg, FileKind.Binary -> Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
            ToolHeader(name, subtitle, onBack) {
                Box {
                    HeaderAction(ZIcons.More, "More", onClick = { overflow = true })
                    ActionMenu(overflow, { overflow = false }, common)
                }
            }
            Box(Modifier.weight(1f).fillMaxWidth()) {
                when (kind) {
                    FileKind.Pdf -> PdfViewer(model, ref, path)
                    FileKind.Image -> ImageViewer(model, ref, path)
                    FileKind.Svg -> WorkspaceWeb(model, Browser.workspaceUrl(ref, path), null)
                    else -> BinaryInfo(name, null, onSave = { model.downloads.saveFile(ref, path) }, onOpenWith = {
                        scope.launch { runCatching { openWith(context, model, ref, path) }.onFailure { toast(context, it.userMessage()) } }
                    })
                }
                DownloadsStrip(model, Modifier.align(Alignment.BottomCenter).navigationBarsPadding())
            }
        }
        else -> TextFile(model, ref, path, kind, name, subtitle, common, onBack, onBrowser)
    }
}

private sealed interface Loaded {
    data object Loading : Loaded
    data class Failed(val message: String) : Loaded
    data class Ready(val file: FileText) : Loaded
}

@Composable
private fun TextFile(
    model: AppModel,
    ref: WorkspaceRef,
    path: String,
    kind: FileKind,
    name: String,
    subtitle: String,
    common: List<MenuAction>,
    onBack: () -> Unit,
    onBrowser: (String) -> Unit,
) {
    val api = model.workspaceApi
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    var loaded by remember(path) { mutableStateOf<Loaded>(Loaded.Loading) }
    var editing by remember { mutableStateOf(false) }
    var buffer by remember { mutableStateOf(TextFieldValue("")) }
    var preview by remember { mutableStateOf(kind == FileKind.Markdown) }
    var wrap by remember { mutableStateOf(kind == FileKind.Markdown) }
    var saving by remember { mutableStateOf(false) }
    var conflict by remember { mutableStateOf<String?>(null) }
    var changedOnDisk by remember { mutableStateOf(false) }
    var confirmDiscard by remember { mutableStateOf(false) }
    var overflow by remember { mutableStateOf(false) }

    val file = (loaded as? Loaded.Ready)?.file
    val dirty = editing && file != null && buffer.text != file.text

    fun load() {
        scope.launch {
            loaded = runCatching { api.read(ref, path) }.fold({ Loaded.Ready(it) }, { Loaded.Failed(it.userMessage()) })
            (loaded as? Loaded.Ready)?.file?.text?.let { buffer = TextFieldValue(it) }
            changedOnDisk = false
        }
    }
    LaunchedEffect(ref, path) { load() }

    // The device's file watcher: reload a clean buffer, flag a dirty one.
    DisposableEffect(ref, path) {
        var stream: HostStream? = null
        var baseline = true
        val job = scope.launch {
            stream = runCatching {
                api.watchFiles(ref, scope, onItem = { frame ->
                    if (frame.optBoolean("resyncRequired")) {
                        if (baseline) {
                            baseline = false
                            return@watchFiles
                        }
                    } else {
                        val changes = frame.optJSONArray("changes")
                        val touched = (0 until (changes?.length() ?: 0)).any {
                            val c = changes!!.getJSONObject(it)
                            c.optString("path") == path || c.optString("oldPath") == path
                        }
                        if (!touched) return@watchFiles
                    }
                    if (saving) return@watchFiles
                    scope.launch {
                        val fresh = runCatching { api.read(ref, path) }.getOrNull() ?: return@launch
                        val current = (loaded as? Loaded.Ready)?.file ?: return@launch
                        if (fresh.contentHash == current.contentHash) return@launch
                        if (editing && buffer.text != current.text) {
                            changedOnDisk = true
                        } else {
                            loaded = Loaded.Ready(fresh)
                            // Keep the caret where it was (clamped) on a live refresh.
                            fresh.text?.let { buffer = TextFieldValue(it, androidx.compose.ui.text.TextRange(buffer.selection.start.coerceIn(0, it.length))) }
                        }
                    }
                }, onEnd = {})
            }.getOrNull()
        }
        onDispose {
            job.cancel()
            stream?.cancel()
            stream?.destroy()
        }
    }

    val fb = LocalFeedback.current
    fun save(overwrite: Boolean = false) {
        val base = file ?: return
        saving = true
        scope.launch {
            try {
                val target = if (overwrite) api.read(ref, path) else base
                when (val r = api.write(ref, target, buffer.text)) {
                    is SaveResult.Written -> {
                        loaded = Loaded.Ready(base.copy(text = buffer.text, contentHash = r.contentHash, checkoutId = target.checkoutId))
                        changedOnDisk = false
                        fb.both(Haptic.Confirm, Cue.Select)
                        toast(context, "Saved $name", error = false)
                    }
                    is SaveResult.Conflict -> {
                        conflict = r.reason
                        fb.both(Haptic.Error, Cue.Error)
                    }
                }
            } catch (e: Exception) {
                toast(context, "Couldn't save: ${e.userMessage()}")
            } finally {
                saving = false
            }
        }
    }

    BackHandler(enabled = dirty) { confirmDiscard = true }

    // Highlighting follows the buffer (debounced while typing).
    var highlighted by remember { mutableStateOf<HighlightedSource?>(null) }
    val source = if (editing) buffer.text else file?.text
    LaunchedEffect(source) {
        val text = source ?: return@LaunchedEffect
        if (editing) delay(350)
        highlighted = withContext(Dispatchers.Default) { runCatching { highlightSource(path, text) }.getOrNull() }
    }

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        ToolHeader(name, subtitle, onBack = { if (dirty) confirmDiscard = true else onBack() }) {
            if (kind == FileKind.Markdown && !editing) HeaderAction(if (preview) ZIcons.Code else ZIcons.Eye, if (preview) "Show source" else "Preview", onClick = { preview = !preview }, selected = preview)
            if (kind == FileKind.Html && !editing) HeaderAction(ZIcons.Globe, "Open in browser", onClick = { onBrowser(Browser.workspaceUrl(ref, path)) })
            if (file?.editable == true) {
                if (editing) {
                    HeaderAction(ZIcons.Save, "Save", onClick = { save() }, selected = dirty)
                } else {
                    HeaderAction(ZIcons.Rename, "Edit", onClick = {
                        editing = true
                        preview = false
                        buffer = TextFieldValue(file.text ?: "")
                    })
                }
            }
            Box {
                HeaderAction(ZIcons.More, "More", onClick = { overflow = true })
                ActionMenu(
                    overflow,
                    { overflow = false },
                    listOfNotNull(
                        if (editing) MenuAction("Stop editing", ZIcons.Close) { if (dirty) confirmDiscard = true else editing = false } else null,
                        if (!editing) MenuAction(if (wrap) "Don't wrap lines" else "Wrap lines", ZIcons.WrapText) { wrap = !wrap } else null,
                        MenuAction("Reload", ZIcons.Refresh) { load() },
                    ) + common,
                )
            }
        }
        if (changedOnDisk) StatusBanner("Changed on ${ref.deviceName ?: "the device"} since you opened it", "Reload" to { load(); editing = false })
        file?.readOnlyReason?.let { reason ->
            val text = when (reason) {
                "tooLarge" -> "Read-only: too large to edit here"
                "binary" -> "Binary file"
                "mixedLineEndings" -> "Read-only: mixed line endings"
                "unsupportedEncoding" -> "Read-only: not UTF-8"
                "symlink" -> "Read-only: symbolic link"
                "permissionDenied" -> "Read-only: permission denied"
                else -> "Read-only"
            }
            if (reason != "binary") StatusBanner(text, null)
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            when (val l = loaded) {
                Loaded.Loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { LoadingIndicator() }
                is Loaded.Failed -> Column(Modifier.fillMaxSize().padding(32.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Hint(l.message)
                    TextButton(onClick = tapAction { load() }) { Text("Try again") }
                }
                is Loaded.Ready -> when {
                    l.file.binary || l.file.text == null -> BinaryInfo(name, l.file.size, onSave = { model.downloads.saveFile(ref, path) }, onOpenWith = {
                        scope.launch { runCatching { openWith(context, model, ref, path) }.onFailure { toast(context, it.userMessage()) } }
                    })
                    editing -> CodeEditor(buffer, { buffer = it }, highlighted)
                    preview && kind == FileKind.Markdown -> WorkspaceWeb(model, Browser.workspaceUrl(ref, path), markdownPage(l.file.text))
                    else -> CodeView(l.file.text, highlighted, wrap)
                }
            }
            DownloadsStrip(model, Modifier.align(Alignment.BottomCenter).navigationBarsPadding().imePadding())
        }
    }

    conflict?.let { reason ->
        AlertDialog(
            onDismissRequest = { conflict = null },
            title = { Text(if (reason == "deleted") "File was deleted" else "File changed on the device") },
            text = {
                OpenCloseFeedback()
                Text(
                    if (reason == "deleted") "$name no longer exists on ${ref.deviceName ?: "the device"}. Your edits are still here."
                    else "Someone (maybe the agent) changed $name after you opened it. Overwrite their version with yours, or reload theirs?",
                )
            },
            confirmButton = {
                if (reason != "deleted") TextButton(onClick = tapAction { conflict = null; save(overwrite = true) }) { Text("Overwrite") }
            },
            dismissButton = {
                Row {
                    TextButton(onClick = tapAction { conflict = null; editing = false; load() }) { Text("Reload") }
                    TextButton(onClick = tapAction { conflict = null }) { Text("Keep editing") }
                }
            },
        )
    }
    if (confirmDiscard) {
        AlertDialog(
            onDismissRequest = { confirmDiscard = false },
            title = { Text("Discard your changes?") },
            text = {
                OpenCloseFeedback()
                Text("$name has unsaved edits.")
            },
            confirmButton = { TextButton(onClick = tapAction { confirmDiscard = false; save() }) { Text("Save") } },
            dismissButton = {
                Row {
                    TextButton(onClick = feedbackAction(Haptic.Heavy, Cue.Delete) {
                        confirmDiscard = false
                        editing = false
                        buffer = TextFieldValue(file?.text ?: "")
                    }) { Text("Discard") }
                    TextButton(onClick = tapAction { confirmDiscard = false }) { Text("Cancel") }
                }
            },
        )
    }
}

private val codeStyle = TextStyle(fontFamily = GeistMono, fontSize = 12.5.sp, lineHeight = 19.sp)

/** Highlight spans (UTF-16 offsets per line) → styled text for one line. */
private fun styledLine(line: String, spans: List<uniffi.zeron_core.CodeSpan>?, palette: TranscriptPalette, base: Color): AnnotatedString = buildAnnotatedString {
    append(line)
    addStyle(SpanStyle(color = base), 0, line.length)
    for (s in spans.orEmpty()) {
        val start = s.start.toInt().coerceIn(0, line.length)
        val end = s.end.toInt().coerceIn(start, line.length)
        if (end > start) addStyle(SpanStyle(color = Color(palette[s.color])), start, end)
    }
}

@Composable
private fun rememberCodePalette(): TranscriptPalette {
    val dark = LocalDarkTheme.current
    val scheme = MaterialTheme.colorScheme
    return remember(dark, scheme) { TranscriptPalette(dark, scheme) }
}

/** Read-only code: virtualized lines, a line-number gutter, wrap or scroll sideways. */
@Composable
private fun CodeView(text: String, highlighted: HighlightedSource?, wrap: Boolean) {
    val lines = remember(text) { text.split('\n').let { if (it.size > 1 && it.last().isEmpty()) it.dropLast(1) else it } }
    val palette = rememberCodePalette()
    val base = Color(palette[uniffi.zeron_core.ColorRole.CODE_TEXT])
    val gutter = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.6f)
    val digits = lines.size.toString().length
    val longest = remember(lines) { lines.maxOfOrNull { it.length } ?: 0 }
    val horizontal = rememberScrollState()
    val gutterWidth = (digits * 8 + 16).dp
    val content = @Composable { m: Modifier ->
        SelectionContainer {
            LazyColumn(m, contentPadding = PaddingValues(top = 8.dp, bottom = 140.dp)) {
                itemsIndexed(lines) { i, line ->
                    Row {
                        Text(
                            "${i + 1}",
                            style = codeStyle,
                            color = gutter,
                            textAlign = TextAlign.End,
                            modifier = Modifier.width(gutterWidth).padding(end = 10.dp),
                        )
                        Text(
                            styledLine(line, highlighted?.lines?.getOrNull(i)?.spans, palette, base),
                            style = codeStyle,
                            softWrap = wrap,
                            modifier = if (wrap) Modifier.weight(1f).padding(end = 12.dp) else Modifier.padding(end = 24.dp),
                        )
                    }
                }
            }
        }
    }
    if (wrap) {
        content(Modifier.fillMaxSize())
    } else {
        // Wide enough for the longest line: Geist Mono advances ~0.6 em.
        val width = (gutterWidth.value + longest * 7.6f + 40f).dp
        Box(Modifier.fillMaxSize().horizontalScroll(horizontal)) {
            content(Modifier.widthIn(min = 360.dp).width(width).fillMaxSize())
        }
    }
}

/** The editor: the whole buffer in one field (no wrap), highlighted as you type. */
@Composable
private fun CodeEditor(value: TextFieldValue, onChange: (TextFieldValue) -> Unit, highlighted: HighlightedSource?) {
    val palette = rememberCodePalette()
    val base = Color(palette[uniffi.zeron_core.ColorRole.CODE_TEXT])
    val gutter = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.6f)
    val lineCount = remember(value.text) { value.text.count { it == '\n' } + 1 }
    val numbers = remember(lineCount) { (1..lineCount).joinToString("\n") }
    val digits = lineCount.toString().length
    val transformation = remember(highlighted, palette) {
        VisualTransformation { text ->
            val styled = buildAnnotatedString {
                append(text.text)
                addStyle(SpanStyle(color = base), 0, text.length)
                val lines = highlighted?.lines
                if (lines != null) {
                    var start = 0
                    for ((i, line) in text.text.split('\n').withIndex()) {
                        for (s in lines.getOrNull(i)?.spans.orEmpty()) {
                            val a = (start + s.start.toInt()).coerceIn(0, text.length)
                            val b = (start + s.end.toInt()).coerceIn(a, (start + line.length).coerceAtMost(text.length))
                            if (b > a) addStyle(SpanStyle(color = Color(palette[s.color])), a, b)
                        }
                        start += line.length + 1
                    }
                }
            }
            TransformedText(styled, OffsetMapping.Identity)
        }
    }
    Row(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).imePadding().padding(top = 8.dp, bottom = 140.dp)) {
        Text(numbers, style = codeStyle, color = gutter, textAlign = TextAlign.End, modifier = Modifier.width((digits * 8 + 16).dp).padding(end = 10.dp))
        Box(Modifier.weight(1f).horizontalScroll(rememberScrollState())) {
            BasicTextField(
                value,
                onChange,
                textStyle = codeStyle.copy(color = base),
                cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                visualTransformation = transformation,
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false),
                modifier = Modifier.widthIn(min = 320.dp).padding(end = 24.dp),
            )
        }
    }
}

/** Markdown as a themed page (scripts off); relative images resolve in the workspace. */
@Composable
private fun markdownPage(text: String): String {
    val scheme = MaterialTheme.colorScheme
    val dark = LocalDarkTheme.current
    fun css(c: Color) = "#%06X".format(c.toArgb() and 0xFFFFFF)
    val body = remember(text) { markdownHtml(text) }
    return """<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<style>
:root{color-scheme:${if (dark) "dark" else "light"}}
body{margin:0;padding:18px 20px 120px;background:${css(scheme.background)};color:${css(scheme.onSurface)};font:16px/1.6 -apple-system,Roboto,sans-serif;word-wrap:break-word}
a{color:${css(scheme.primary)}} img{max-width:100%;border-radius:8px}
h1,h2,h3{line-height:1.25;margin:1.4em 0 .5em} h1{font-size:1.7em} h2{font-size:1.35em;border-bottom:1px solid ${css(scheme.outlineVariant)};padding-bottom:.3em}
code{font-family:monospace;font-size:.88em;background:${css(scheme.surfaceContainerHigh)};padding:.15em .35em;border-radius:5px}
pre{background:${css(scheme.surfaceContainer)};padding:12px 14px;border-radius:12px;overflow:auto}
pre code{background:none;padding:0}
blockquote{margin:0;padding:0 14px;border-left:3px solid ${css(scheme.primary)};color:${css(scheme.onSurfaceVariant)}}
table{border-collapse:collapse;display:block;overflow:auto} th,td{border:1px solid ${css(scheme.outlineVariant)};padding:6px 10px}
hr{border:0;border-top:1px solid ${css(scheme.outlineVariant)}}
input[type=checkbox]{margin-right:6px}
</style></head><body>$body</body></html>"""
}

/** A WebView on a workspace URL (or [html] with that URL as its base), served by [Browser]. */
@SuppressLint("SetJavaScriptEnabled")
@Composable
fun WorkspaceWeb(model: AppModel, url: String, html: String?) {
    AndroidView(
        factory = { ctx ->
            WebView(ctx).apply {
                settings.javaScriptEnabled = html == null
                settings.builtInZoomControls = true
                settings.displayZoomControls = false
                settings.allowFileAccess = false
                setBackgroundColor(android.graphics.Color.TRANSPARENT)
                webViewClient = object : WebViewClient() {
                    override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? =
                        if (Browser.isWorkspace(request.url.toString())) Browser.intercept(model, request.url) else null
                }
            }
        },
        update = { web ->
            val key = url + (html?.hashCode() ?: 0)
            if (web.tag != key) {
                web.tag = key
                if (html != null) web.loadDataWithBaseURL(url, html, "text/html", "utf-8", null) else web.loadUrl(url)
            }
        },
        modifier = Modifier.fillMaxSize(),
    )
}
