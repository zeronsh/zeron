package sh.zeron.android.tools

import sh.zeron.android.feedback.tapAction
import sh.zeron.android.feedback.feedbackAction
import android.annotation.SuppressLint
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.net.Uri
import android.print.PrintAttributes
import android.print.PrintManager
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.FloatingToolbarDefaults
import androidx.compose.material3.HorizontalFloatingToolbar
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearWavyProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.launch
import org.json.JSONObject
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import sh.zeron.android.ui.ActionMenu
import sh.zeron.android.ui.MenuAction
import sh.zeron.android.ui.TonalCircleButton
import uniffi.zeron_core.HostStream

/**
 * The in-app browser (desktop browser tabs): URL bar, back/forward,
 * reload/stop, the session's dev-server previews, open externally, and
 * print/save as PDF. Workspace pages and previews are explained in [Browser].
 */
@SuppressLint("SetJavaScriptEnabled")
@Composable
fun BrowserScreen(model: AppModel, ref: WorkspaceRef?, initialUrl: String?, onBack: () -> Unit, onOpenFile: (String) -> Unit = {}) {
    val context = LocalContext.current
    val clipboard = LocalClipboardManager.current
    val scope = rememberCoroutineScope()
    var url by remember { mutableStateOf(initialUrl ?: "") }
    var title by remember { mutableStateOf<String?>(null) }
    var progress by remember { mutableFloatStateOf(0f) }
    var loading by remember { mutableStateOf(false) }
    var canBack by remember { mutableStateOf(false) }
    var canForward by remember { mutableStateOf(false) }
    var editing by remember { mutableStateOf(initialUrl == null) }
    var address by remember { mutableStateOf(initialUrl ?: "") }
    var overflow by remember { mutableStateOf(false) }
    var showPreviews by remember { mutableStateOf(initialUrl == null && ref?.chatId != null) }
    var previews by remember { mutableStateOf<Browser.Previews?>(null) }
    /** A PDF the page opened (WebView can't render them): shown natively on top. */
    var pdf by remember { mutableStateOf<String?>(null) }
    val focus = LocalFocusManager.current

    val web = remember {
        WebView(context).apply {
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true
            settings.loadWithOverviewMode = true
            settings.useWideViewPort = true
            settings.builtInZoomControls = true
            settings.displayZoomControls = false
            settings.mediaPlaybackRequiresUserGesture = false
            settings.mixedContentMode = WebSettings.MIXED_CONTENT_COMPATIBILITY_MODE
            settings.allowFileAccess = false
            webViewClient = object : WebViewClient() {
                override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? =
                    if (Browser.isWorkspace(request.url.toString())) Browser.intercept(model, request.url) else null

                override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                    // A workspace PDF/image/binary opens in the native viewer (WebView can't show PDFs).
                    Browser.resolve(request.url)?.let { (target, path) ->
                        val kind = FileKind.of(path)
                        if (target == ref && (kind == FileKind.Pdf || kind == FileKind.Binary)) {
                            onOpenFile(path)
                            return true
                        }
                    }
                    val scheme = request.url.scheme ?: return false
                    if (scheme == "http" || scheme == "https" || scheme == "about" || scheme == "data" || scheme == "blob") return false
                    runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, request.url)) }
                    return true
                }

                override fun onPageStarted(view: WebView, u: String?, favicon: Bitmap?) {
                    loading = true
                    url = u ?: url
                    if (!editing) address = u ?: address
                }

                override fun onPageFinished(view: WebView, u: String?) {
                    loading = false
                    url = u ?: url
                    title = view.title?.takeIf { it.isNotBlank() && it != u }
                    canBack = view.canGoBack()
                    canForward = view.canGoForward()
                }

                override fun doUpdateVisitedHistory(view: WebView, u: String?, isReload: Boolean) {
                    url = u ?: url
                    if (!editing) address = u ?: address
                    canBack = view.canGoBack()
                    canForward = view.canGoForward()
                }
            }
            // Downloads (a PDF on a dev server, a zip…): hand them to the system.
            setDownloadListener { url, _, _, mime, _ ->
                when {
                    mime == "application/pdf" || url.substringBefore('?').endsWith(".pdf", ignoreCase = true) -> pdf = url
                    url.startsWith("http") -> openExternally(context, url)
                }
            }
            webChromeClient = object : WebChromeClient() {
                override fun onProgressChanged(view: WebView, newProgress: Int) {
                    progress = newProgress / 100f
                }

                override fun onReceivedTitle(view: WebView, t: String?) {
                    title = t?.takeIf { it.isNotBlank() && !it.startsWith("http") }
                }
            }
            if (initialUrl != null) loadUrl(initialUrl)
        }
    }
    DisposableEffect(web) {
        onDispose {
            web.stopLoading()
            web.destroy()
        }
    }

    // Previews: the dev servers the session's own device discovered.
    DisposableEffect(ref?.chatId) {
        var stream: HostStream? = null
        val chat = ref?.chatId
        val device = ref?.deviceId
        val job = if (chat != null && device != null) scope.launch {
            stream = runCatching {
                model.workspaceApi.watch(device, WorkspaceApi.WATCH_PREVIEWS, JSONObject().put("chatId", chat), scope, { previews = Browser.previews(it) })
            }.onFailure { previews = Browser.Previews(emptyList(), 7331, it.message) }.getOrNull()
        } else null
        onDispose {
            job?.cancel()
            stream?.cancel()
            stream?.destroy()
        }
    }

    fun go(input: String) {
        val target = Browser.normalize(input)
        editing = false
        focus.clearFocus()
        address = target
        web.loadUrl(target)
    }

    BackHandler(enabled = true) {
        when {
            editing && url.isNotEmpty() -> {
                editing = false
                focus.clearFocus()
            }
            web.canGoBack() -> web.goBack()
            else -> onBack()
        }
    }

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Row(
            Modifier.fillMaxWidth().statusBarsPadding().padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            TonalCircleButton(ZIcons.Close, "Close browser", onClick = onBack, size = 44.dp, container = MaterialTheme.colorScheme.surfaceContainerHighest)
            Spacer(Modifier.width(10.dp))
            AddressBar(
                value = if (editing) address else (title?.let { "$it  ·  ${Browser.display(url)}" } ?: Browser.display(url)),
                editing = editing,
                onEdit = {
                    if (!editing) {
                        editing = true
                        address = if (Browser.isWorkspace(url)) url else url
                    }
                },
                onChange = { address = it },
                onGo = { go(address) },
                modifier = Modifier.weight(1f),
            )
            Spacer(Modifier.width(10.dp))
            Box {
                TonalCircleButton(ZIcons.More, "More", onClick = { overflow = true }, size = 44.dp, container = MaterialTheme.colorScheme.surfaceContainerHighest)
                ActionMenu(
                    overflow,
                    { overflow = false },
                    listOfNotNull(
                        if (!Browser.isWorkspace(url) && url.startsWith("http")) MenuAction("Open in another app", ZIcons.Link) { openExternally(context, url) } else null,
                        MenuAction("Copy link", ZIcons.Copy, haptic = sh.zeron.android.feedback.Haptic.Confirm, cue = sh.zeron.android.feedback.Cue.Copy) { clipboard.setText(AnnotatedString(url)) },
                        MenuAction("Print or save as PDF", ZIcons.Save) { print(context, web, title ?: Browser.display(url)) },
                        MenuAction("Reload", ZIcons.Refresh, haptic = sh.zeron.android.feedback.Haptic.Select, cue = sh.zeron.android.feedback.Cue.Refresh) { web.reload() },
                    ),
                )
            }
        }
        AnimatedVisibility(loading) {
            LinearWavyProgressIndicator(progress = { progress.coerceAtLeast(0.05f) }, modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp))
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            AndroidView({ web }, Modifier.fillMaxSize())
            if (url.isEmpty() && !loading) {
                Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).padding(28.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
                    ZIcon(ZIcons.Globe, null, Modifier.size(40.dp), tint = MaterialTheme.colorScheme.primary)
                    Spacer(Modifier.height(12.dp))
                    Text("Open a page", style = MaterialTheme.typography.titleLarge)
                    Spacer(Modifier.height(6.dp))
                    Text(
                        "Type a URL or a port (8000 → localhost:8000). Dev servers the agent starts show up in Previews.",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            HorizontalFloatingToolbar(
                expanded = true,
                modifier = Modifier.align(Alignment.BottomCenter).imePadding().navigationBarsPadding().padding(bottom = 12.dp),
                colors = FloatingToolbarDefaults.standardFloatingToolbarColors(),
            ) {
                IconButton(onClick = tapAction { web.goBack() }, enabled = canBack) { ZIcon(ZIcons.Back, "Back", Modifier.size(22.dp)) }
                IconButton(onClick = tapAction { web.goForward() }, enabled = canForward) { ZIcon(ZIcons.Forward, "Forward", Modifier.size(22.dp)) }
                IconButton(onClick = sh.zeron.android.feedback.feedbackAction(sh.zeron.android.feedback.Haptic.Select, if (loading) sh.zeron.android.feedback.Cue.Close else sh.zeron.android.feedback.Cue.Refresh) { if (loading) web.stopLoading() else web.reload() }) {
                    ZIcon(if (loading) ZIcons.Stop else ZIcons.Refresh, if (loading) "Stop" else "Reload", Modifier.size(22.dp))
                }
                if (ref?.chatId != null) {
                    IconButton(onClick = tapAction { showPreviews = true }) {
                        val count = previews?.services?.size ?: 0
                        BadgedBox(badge = { if (count > 0) Badge { Text("$count") } }) { ZIcon(ZIcons.Play, "Previews", Modifier.size(22.dp)) }
                    }
                }
                IconButton(onClick = tapAction { openExternally(context, url) }, enabled = url.startsWith("http") && !Browser.isWorkspace(url)) {
                    ZIcon(ZIcons.Link, "Open in another app", Modifier.size(22.dp))
                }
            }
        }
    }

    pdf?.let { link ->
        BackHandler { pdf = null }
        Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
            ToolHeader(link.substringBefore('?').substringAfterLast('/').ifEmpty { "Document" }, Browser.display(link), onBack = { pdf = null }) {
                HeaderAction(ZIcons.Link, "Open in another app", onClick = { openExternally(context, link) })
            }
            Box(Modifier.weight(1f).fillMaxWidth()) {
                PdfFrom(link) { download(context, link) }
            }
        }
    }

    if (showPreviews) {
        ModalBottomSheet(onDismissRequest = { showPreviews = false }) {
            sh.zeron.android.feedback.OpenCloseFeedback()
            PreviewsSheet(previews, ref) { target ->
                showPreviews = false
                go(target)
            }
        }
    }
}

@Composable
private fun AddressBar(value: String, editing: Boolean, onEdit: () -> Unit, onChange: (String) -> Unit, onGo: () -> Unit, modifier: Modifier) {
    Surface(shape = RoundedCornerShape(50), color = MaterialTheme.colorScheme.surfaceContainerHigh, modifier = modifier.height(44.dp)) {
        Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
            ZIcon(ZIcons.Globe, null, Modifier.size(16.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.width(8.dp))
            if (editing) {
                BasicTextField(
                    value,
                    onChange,
                    singleLine = true,
                    textStyle = MaterialTheme.typography.bodyMedium.copy(color = MaterialTheme.colorScheme.onSurface),
                    cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go, autoCorrectEnabled = false),
                    keyboardActions = KeyboardActions(onGo = { onGo() }),
                    modifier = Modifier.weight(1f).onFocusChanged { },
                )
            } else {
                Text(
                    value.ifEmpty { "Search or type a URL" },
                    style = MaterialTheme.typography.bodyMedium,
                    color = if (value.isEmpty()) MaterialTheme.colorScheme.onSurfaceVariant else MaterialTheme.colorScheme.onSurface,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f).clickable(onClick = onEdit),
                )
            }
        }
    }
}

/**
 * The session's dev servers, as its device reports them. This app has no
 * preview proxy of its own, so a server opens straight from the device over
 * the network: its address is asked for once per device and remembered
 * (`10.0.2.2` is the emulator's host). The server must listen beyond
 * localhost.
 */
@Composable
private fun PreviewsSheet(previews: Browser.Previews?, ref: WorkspaceRef?, open: (String) -> Unit) {
    val context = LocalContext.current
    val prefs = remember { context.getSharedPreferences("browser", 0) }
    val key = "address.${ref?.deviceId.orEmpty()}"
    var address by remember(key) { mutableStateOf(prefs.getString(key, null).orEmpty()) }
    val host = Browser.previewHost(address)
    Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp).padding(bottom = 28.dp)) {
        Text("Previews", style = MaterialTheme.typography.titleLargeEmphasized)
        Spacer(Modifier.height(4.dp))
        Text(
            "Dev servers running in ${ref?.title ?: "this project"}${ref?.deviceName?.let { " on $it" } ?: ""}.",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Spacer(Modifier.height(12.dp))
        OutlinedTextField(
            address,
            {
                address = it
                prefs.edit().putString(key, it.trim()).apply()
            },
            label = { Text("${ref?.deviceName ?: "The device"}'s address on your network") },
            placeholder = { Text("192.168.1.20") },
            supportingText = { Text("Servers open from it directly and must listen beyond localhost.") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Done, autoCorrectEnabled = false),
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(12.dp))
        val services = previews?.services.orEmpty()
        if (previews == null) Text("Looking for servers…", style = MaterialTheme.typography.bodyMedium)
        else if (services.isEmpty()) {
            Text(
                previews.error ?: "Nothing yet. Start one in the terminal or ask the agent (e.g. python3 -m http.server 8000).",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        services.forEachIndexed { i, s ->
            val url = host?.takeIf { s.port > 0 }?.let { "http://$it:${s.port}" }
            Surface(
                onClick = tapAction { url?.let(open) },
                enabled = url != null,
                shape = sh.zeron.android.ui.segmentShape(i, services.size),
                color = MaterialTheme.colorScheme.surfaceContainerHigh,
                modifier = Modifier.fillMaxWidth().padding(vertical = 1.dp),
            ) {
                Row(Modifier.padding(horizontal = 16.dp, vertical = 14.dp), verticalAlignment = Alignment.CenterVertically) {
                    ZIcon(ZIcons.Globe, null, Modifier.size(22.dp), tint = if (url != null) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline)
                    Spacer(Modifier.width(14.dp))
                    Column(Modifier.weight(1f)) {
                        Text(s.name, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(url?.removePrefix("http://") ?: "port ${s.port}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                }
            }
        }
    }
}

private fun openExternally(context: Context, url: String) {
    runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url))) }
        .onFailure { toast(context, "No app can open this link") }
}

private fun print(context: Context, web: WebView, name: String) {
    val manager = context.getSystemService(PrintManager::class.java) ?: return
    val job = name.replace(Regex("[^A-Za-z0-9 ._-]"), "_").take(60).ifBlank { "Page" }
    manager.print(job, web.createPrintDocumentAdapter(job), PrintAttributes.Builder().build())
}

/** Fetch a page's PDF into the cache (plain HTTP GET; dev servers and public links). */
private suspend fun download(context: Context, url: String): java.io.File = kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) {
    val dir = java.io.File(context.cacheDir, "pdf").apply { mkdirs() }
    val file = java.io.File(dir, "page-" + Integer.toHexString(url.hashCode()) + ".pdf")
    // `<device>.<project>.localhost` resolves inside WebView (Chromium) but
    // not through the platform resolver: dial loopback and keep the Host.
    val parsed = java.net.URL(url)
    val preview = parsed.host.endsWith(".localhost")
    val target = if (preview) java.net.URL(parsed.protocol, "127.0.0.1", parsed.port, parsed.file) else parsed
    val connection = target.openConnection() as java.net.HttpURLConnection
    if (preview) connection.setRequestProperty("Host", "${parsed.host}:${parsed.port}")
    try {
        connection.connectTimeout = 15_000
        connection.readTimeout = 30_000
        if (connection.responseCode !in 200..299) error("The server answered ${connection.responseCode}.")
        connection.inputStream.use { input -> file.outputStream().use { input.copyTo(it) } }
    } finally {
        connection.disconnect()
    }
    file
}
