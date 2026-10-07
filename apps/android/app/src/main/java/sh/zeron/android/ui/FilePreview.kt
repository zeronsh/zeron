package sh.zeron.android.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.zeron.android.R
import sh.zeron.android.design.BackButton
import sh.zeron.android.design.consumeBlankTaps
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.Glyphs
import sh.zeron.android.design.LocalZeronColors
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronType
import sh.zeron.android.design.glassSurface
import uniffi.zeron_core.CoreException
import uniffi.zeron_core.WorkspaceFile

/** What the file preview shows: pure, so the states are unit-testable. */
internal sealed interface FilePreviewState {
    data object Loading : FilePreviewState
    data class Text(val file: WorkspaceFile, val markdown: Boolean) : FilePreviewState
    data class Binary(val file: WorkspaceFile) : FilePreviewState
    /** [outside]: the link points outside the project folder (not retryable). */
    data class Failed(val outside: Boolean, val message: String?) : FilePreviewState
}

internal object FilePreviews {
    private val markdownExt = setOf("md", "markdown", "mdx", "mdown")

    /** The name shown in the header: the link's last path segment. */
    fun title(url: String): String {
        val path = url.substringBefore('#').substringBefore('?').trimEnd('/', '\\')
        val last = path.substringAfterLast('/').substringAfterLast('\\').ifEmpty { path }
        // Drop an editor line ref (`report.md:12`, `x.rs:3:7`).
        val name = last.replace(Regex("(:\\d+){1,2}$"), "")
        return runCatching { java.net.URLDecoder.decode(name.replace("+", "%2B"), "UTF-8") }.getOrDefault(name)
    }

    fun isMarkdown(path: String): Boolean = title(path).substringAfterLast('.', "").lowercase() in markdownExt

    /** The engine's resolved `file.path` is the primary signal; fall back to
     *  the link's own name so an odd path can't hide a `.md` preview. */
    fun loaded(file: WorkspaceFile, url: String): FilePreviewState =
        if (file.text == null) FilePreviewState.Binary(file)
        else FilePreviewState.Text(file, isMarkdown(file.path) || isMarkdown(url))

    fun failed(t: Throwable): FilePreviewState = when (t) {
        is CoreException.InvalidArgument -> FilePreviewState.Failed(outside = true, message = null)
        else -> FilePreviewState.Failed(outside = false, message = t.message)
    }
}

/**
 * Full-screen preview of a file an agent linked in its reply (report.md,
 * /abs/checkout/docs/x.md:12, file:///…): read from the chat's workspace
 * on its computer, Markdown rendered, anything else as monospace text,
 * with copy and share. [load] defaults to the core's `readFileLink`.
 */
@Composable
internal fun FilePreview(
    url: String,
    onClose: () -> Unit,
    load: suspend () -> WorkspaceFile,
    onCopied: () -> Unit = {},
) {
    val colors = LocalZeronColors.current
    val context = LocalContext.current
    var attempt by remember(url) { mutableIntStateOf(0) }
    var state by remember(url) { mutableStateOf<FilePreviewState>(FilePreviewState.Loading) }
    BackHandler(onBack = onClose)
    LaunchedEffect(url, attempt) {
        state = FilePreviewState.Loading
        state = try {
            FilePreviews.loaded(load(), url)
        } catch (t: Throwable) {
            if (t is kotlinx.coroutines.CancellationException) throw t
            FilePreviews.failed(t)
        }
    }
    val title = FilePreviews.title(url)
    val text = (state as? FilePreviewState.Text)?.file?.text
    Column(
        Modifier.fillMaxSize().background(colors.background).consumeBlankTaps().statusBarsPadding().navigationBarsPadding().testTag("file-preview"),
    ) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            BackButton(colors, onClick = onClose)
            Text(
                title,
                color = colors.text,
                fontFamily = ZeronType.Sans,
                fontWeight = FontWeight.SemiBold,
                fontSize = 17.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                textAlign = TextAlign.Center,
                modifier = Modifier.weight(1f).padding(horizontal = 8.dp),
            )
            HeaderAction(Glyphs.Copy, stringResource(R.string.copy), colors, enabled = text != null) {
                val cm = context.getSystemService(ClipboardManager::class.java)
                cm.setPrimaryClip(ClipData.newPlainText(title, text))
                onCopied()
            }
            Spacer(Modifier.width(8.dp))
            val shareLabel = stringResource(R.string.file_share)
            HeaderAction(Glyphs.Share, shareLabel, colors, enabled = text != null) {
                val send = Intent(Intent.ACTION_SEND).setType("text/plain")
                    .putExtra(Intent.EXTRA_SUBJECT, title)
                    .putExtra(Intent.EXTRA_TEXT, text)
                runCatching { context.startActivity(Intent.createChooser(send, shareLabel)) }
            }
        }
        when (val s = state) {
            FilePreviewState.Loading -> Centered {
                CircularProgressIndicator(color = colors.secondary, strokeWidth = 2.dp, modifier = Modifier.size(22.dp))
                Spacer(Modifier.size(12.dp))
                Note(stringResource(R.string.file_reading), colors)
            }
            is FilePreviewState.Failed -> Centered {
                Note(
                    stringResource(if (s.outside) R.string.file_outside_project else R.string.file_read_failed),
                    colors,
                    strong = true,
                )
                if (!s.outside) {
                    s.message?.let { Spacer(Modifier.size(6.dp)); Note(it, colors) }
                    Spacer(Modifier.size(16.dp))
                    Text(
                        stringResource(R.string.retry_now),
                        color = colors.text,
                        fontFamily = ZeronType.Sans,
                        fontSize = 15.sp,
                        modifier = Modifier
                            .clip(RoundedCornerShape(20.dp))
                            .glassSurface(colors, 20.dp)
                            .clickable { attempt++ }
                            .padding(horizontal = 18.dp, vertical = 10.dp)
                            .testTag("file-retry"),
                    )
                }
            }
            is FilePreviewState.Binary -> Centered {
                val size = android.text.format.Formatter.formatShortFileSize(context, s.file.size.toLong())
                Note(stringResource(R.string.file_binary, size), colors, strong = true)
            }
            is FilePreviewState.Text -> Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
                Text(
                    s.file.path,
                    color = colors.secondary,
                    fontFamily = ZeronType.Mono,
                    fontSize = 12.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.padding(horizontal = 20.dp, vertical = 4.dp),
                )
                if (s.file.truncated) {
                    Text(
                        stringResource(R.string.file_truncated),
                        color = colors.secondary,
                        fontFamily = ZeronType.Sans,
                        fontSize = 13.sp,
                        modifier = Modifier
                            .padding(horizontal = 16.dp, vertical = 6.dp)
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(10.dp))
                            .background(colors.codeBackground)
                            .padding(horizontal = 12.dp, vertical = 8.dp),
                    )
                }
                val body = s.file.text.orEmpty()
                if (s.markdown) {
                    MarkdownText(body, colors, Modifier.padding(horizontal = 20.dp, vertical = 8.dp))
                } else {
                    Text(
                        body,
                        color = colors.text,
                        fontFamily = ZeronType.Mono,
                        fontSize = 12.5.sp,
                        lineHeight = 18.sp,
                        softWrap = false,
                        modifier = Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 8.dp),
                    )
                }
                Spacer(Modifier.size(24.dp))
            }
        }
    }
}

@Composable
private fun HeaderAction(glyph: androidx.compose.ui.graphics.vector.ImageVector, label: String, colors: ZeronColors, enabled: Boolean, onClick: () -> Unit) {
    Box(
        Modifier
            .size(44.dp)
            .glassSurface(colors, 22.dp)
            .clickable(enabled = enabled, onClickLabel = label, role = Role.Button, onClick = onClick)
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) { Glyph(glyph, 18.dp, if (enabled) colors.text else colors.secondary.copy(alpha = 0.5f)) }
}

@Composable
private fun Centered(content: @Composable () -> Unit) {
    Column(
        Modifier.fillMaxSize().padding(horizontal = 32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) { content() }
}

@Composable
private fun Note(text: String, colors: ZeronColors, strong: Boolean = false) {
    Text(
        text,
        color = if (strong) colors.text else colors.secondary,
        fontFamily = ZeronType.Sans,
        fontSize = if (strong) 15.sp else 13.sp,
        textAlign = TextAlign.Center,
    )
}
