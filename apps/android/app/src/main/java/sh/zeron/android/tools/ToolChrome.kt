package sh.zeron.android.tools

import sh.zeron.android.feedback.tapAction
import android.content.Context
import android.widget.Toast
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.expandVertically
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.LinearWavyProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.LocalDarkTheme
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import sh.zeron.android.transcript.IconAssets
import sh.zeron.android.ui.TonalCircleButton
import uniffi.zeron_core.fileIconName
import uniffi.zeron_core.folderIconName

/** The tools' header: back, a centered title/subtitle, round actions (the session screen's chrome). */
@Composable
fun ToolHeader(
    title: String,
    subtitle: String?,
    onBack: () -> Unit,
    color: Color = MaterialTheme.colorScheme.background,
    actions: @Composable RowScope.() -> Unit = {},
) {
    Surface(color = color) {
        Row(
            Modifier.fillMaxWidth().statusBarsPadding().padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            TonalCircleButton(ZIcons.Back, "Back", onClick = onBack, container = MaterialTheme.colorScheme.surfaceContainerHighest)
            Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.titleMediumEmphasized)
                if (subtitle != null) {
                    Text(subtitle, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically, content = actions)
        }
    }
}

/** A round header action in the tools' tone. */
@Composable
fun HeaderAction(icon: Int, label: String, onClick: () -> Unit, selected: Boolean = false) {
    TonalCircleButton(
        icon,
        label,
        onClick = onClick,
        size = 44.dp,
        container = if (selected) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceContainerHighest,
        content = if (selected) MaterialTheme.colorScheme.onPrimaryContainer else MaterialTheme.colorScheme.onSurface,
    )
}

/** The desktop file tree's multi-color icons (bundled PNGs), with a line-icon fallback. */
@Composable
fun FileIcon(path: String, isDir: Boolean, size: Dp = 20.dp) {
    val context = LocalContext.current
    val name = remember(path, isDir) { if (isDir) folderIconName(path.substringAfterLast('/')) else fileIconName(path) }
    val image = remember(name) { IconAssets.load(context, name) }
    if (image != null) {
        Image(image, null, Modifier.size(size))
    } else {
        ZIcon(if (isDir) ZIcons.Folder else ZIcons.Text, null, Modifier.size(size), tint = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/** Git decoration colors (desktop: success / warning / accent / danger). */
@Composable
fun gitColor(kind: GitMark.Kind): Color {
    val dark = LocalDarkTheme.current
    return when (kind) {
        GitMark.Kind.Added, GitMark.Kind.Untracked -> if (dark) Color(0xFF34D399) else Color(0xFF15803D)
        GitMark.Kind.Modified -> if (dark) Color(0xFFFACC15) else Color(0xFFA16207)
        GitMark.Kind.Renamed -> MaterialTheme.colorScheme.primary
        GitMark.Kind.Deleted, GitMark.Kind.Conflict -> MaterialTheme.colorScheme.error
    }
}

/** Save-to-Downloads jobs in flight or just finished, as slim cards. */
@Composable
fun DownloadsStrip(model: AppModel, modifier: Modifier = Modifier) {
    val jobs by model.downloads.jobs.collectAsState()
    val context = LocalContext.current
    AnimatedVisibility(jobs.isNotEmpty(), enter = expandVertically(), exit = shrinkVertically(), modifier = modifier) {
        Column(Modifier.padding(horizontal = 12.dp, vertical = 6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            for (job in jobs.takeLast(3)) {
                Surface(shape = RoundedCornerShape(20.dp), color = MaterialTheme.colorScheme.secondaryContainer, contentColor = MaterialTheme.colorScheme.onSecondaryContainer) {
                    Column(Modifier.fillMaxWidth().padding(start = 16.dp, end = 6.dp, top = 8.dp, bottom = 8.dp)) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            ZIcon(ZIcons.Save, null, Modifier.size(18.dp))
                            Spacer(Modifier.width(10.dp))
                            Column(Modifier.weight(1f)) {
                                val title = when (job.state) {
                                    is Downloads.State.Running -> "Saving ${job.name}"
                                    is Downloads.State.Done -> "Saved ${job.name}"
                                    is Downloads.State.Failed -> "Couldn't save ${job.name}"
                                }
                                Text(title, style = MaterialTheme.typography.labelLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                val detail = when (val s = job.state) {
                                    is Downloads.State.Running -> s.detail
                                    is Downloads.State.Done -> s.detail
                                    is Downloads.State.Failed -> s.message
                                }
                                Text(detail, style = MaterialTheme.typography.bodySmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
                            }
                            when (val s = job.state) {
                                is Downloads.State.Done -> {
                                    TextButton(onClick = tapAction {
                                        runCatching { context.startActivity(model.downloads.openIntent(job.name, s.uri)) }
                                            .onFailure { toast(context, "Nothing can open ${job.name}") }
                                    }) { Text("Open") }
                                    TextButton(onClick = tapAction { model.downloads.dismiss(job.id) }) { Text("Done") }
                                }
                                is Downloads.State.Failed -> TextButton(onClick = tapAction { model.downloads.dismiss(job.id) }) { Text("Dismiss") }
                                is Downloads.State.Running -> Spacer(Modifier.width(10.dp))
                            }
                        }
                        val running = job.state as? Downloads.State.Running
                        if (running != null) {
                            Spacer(Modifier.height(6.dp))
                            val fraction = running.fraction
                            Box(Modifier.padding(end = 10.dp)) {
                                if (fraction != null) LinearWavyProgressIndicator(progress = { fraction }, modifier = Modifier.fillMaxWidth())
                                else LinearWavyProgressIndicator(Modifier.fillMaxWidth())
                            }
                        }
                    }
                }
            }
        }
    }
}

/** A short notice. Notices here are refusals and failures, so they answer with the error cue unless [error] is false. */
fun toast(context: Context, text: String, error: Boolean = true) {
    if (error) sh.zeron.android.feedback.AppFeedback.current.both(sh.zeron.android.feedback.Haptic.Error, sh.zeron.android.feedback.Cue.Error)
    Toast.makeText(context, text, Toast.LENGTH_SHORT).show()
}
