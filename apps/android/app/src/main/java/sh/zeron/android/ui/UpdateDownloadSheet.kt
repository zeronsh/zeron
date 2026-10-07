package sh.zeron.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.zeron.android.R
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronType

/** The download sheet or the mirror list, whichever the model asks for (over any screen). */
@Composable
internal fun UpdateDownloadSheets(model: ZeronModel, colors: ZeronColors) {
    val downloading = model.updateProgress?.let { it < 1f } == true
    when {
        !downloading -> {}
        model.showSourcePicker -> SourcePickerSheet(model, colors)
        model.showDownloadSheet -> DownloadSheet(model, colors)
    }
}

/**
 * Badge tap while an update downloads: how far, from where, how fast, and
 * the two ways out of a bad download, 换个镜像 (Switch mirror) and 取消下载 (Cancel download).
 */
@Composable
internal fun DownloadSheet(model: ZeronModel, colors: ZeronColors) {
    val close = { model.showDownloadSheet = false }
    val p = model.updateProgress ?: 0f
    val status = model.updateStatus
    BottomSheetFrame(colors, onDismiss = close, tag = "download-sheet") {
        Title(colors, stringResource(R.string.download_sheet_title, model.updateRelease?.name.orEmpty()))
        Spacer(Modifier.height(14.dp))
        ProgressBar(colors, p)
        Spacer(Modifier.height(8.dp))
        Row(verticalAlignment = Alignment.CenterVertically) {
            val total = status?.total?.takeIf { it > 0 } ?: model.updateRelease?.size ?: 0L
            val bytes = when {
                status != null && total > 0 -> stringResource(R.string.download_bytes, model.updater.speed(status.done), model.updater.speed(total))
                status != null -> model.updater.speed(status.done)
                else -> ""
            }
            Text(bytes, color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp, modifier = Modifier.weight(1f))
            Text("${(p * 100).toInt()}%", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 13.sp)
        }
        Spacer(Modifier.height(4.dp))
        val source = when {
            status == null -> stringResource(R.string.download_starting)
            status.bytesPerSec <= 0 -> stringResource(R.string.download_connecting, status.source)
            else -> stringResource(R.string.download_from, status.source, model.updater.speed(status.bytesPerSec))
        }
        Text(source, color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Spacer(Modifier.height(18.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            SheetButton(colors, stringResource(R.string.download_switch), Modifier.weight(1f).testTag("download-switch")) {
                model.showDownloadSheet = false
                model.showSourcePicker = true
            }
            SheetButton(colors, stringResource(R.string.download_cancel), Modifier.weight(1f).testTag("download-cancel"), danger = true) {
                model.cancelDownload()
            }
        }
        Spacer(Modifier.height(6.dp))
        Text(
            stringResource(R.string.badge_details),
            color = colors.accent,
            fontFamily = ZeronType.Sans,
            fontSize = 15.sp,
            modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp))
                .clickable { close(); model.showUpdate = true }.padding(horizontal = 4.dp, vertical = 11.dp),
        )
    }
}

/**
 * 换个镜像 (Switch mirror): every source, the one in use ticked, with the last speed or
 * failure seen this session. Picking one carries on from the bytes on disk.
 */
@Composable
internal fun SourcePickerSheet(model: ZeronModel, colors: ZeronColors) {
    val close = { model.showSourcePicker = false }
    val done = model.updateStatus?.done ?: model.updateRelease?.let { model.updater.partialBytes(it) } ?: 0L
    BottomSheetFrame(colors, onDismiss = close, tag = "source-picker") {
        Title(colors, stringResource(R.string.source_picker_title))
        Spacer(Modifier.height(4.dp))
        Text(
            if (done > 0) stringResource(R.string.source_picker_sub, model.updater.speed(done)) else stringResource(R.string.source_picker_sub_empty),
            color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp,
        )
        Spacer(Modifier.height(10.dp))
        model.sourceChoices().forEach { choice ->
            SourceRow(model, colors, choice) {
                if (choice.current) close() else model.switchSource(choice.source.key)
            }
        }
    }
}

@Composable
private fun SourceRow(model: ZeronModel, colors: ZeronColors, choice: ZeronModel.SourceChoice, onClick: () -> Unit) {
    val stat = choice.stat
    val speed = model.updateStatus?.bytesPerSec ?: 0L
    val parts = buildList {
        if (choice.current) add(stringResource(R.string.source_current))
        if (choice.custom) add(stringResource(R.string.source_custom))
        if (choice.source.key == sh.zeron.android.core.UpdateSources.GITHUB) add(stringResource(R.string.source_direct))
        when {
            choice.current && speed > 0 -> add(stringResource(R.string.source_speed_now, model.updater.speed(speed)))
            stat?.error != null -> add(stringResource(R.string.source_failed_last, stat.error))
            stat != null && stat.bytesPerSec > 0 -> add(stringResource(R.string.source_speed_last, model.updater.speed(stat.bytesPerSec)))
            !choice.current -> add(stringResource(R.string.source_untried))
        }
    }
    val failed = !choice.current && stat?.error != null
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(if (choice.current) colors.rowActive else Color.Transparent)
            .clickable(onClick = onClick).padding(horizontal = 12.dp, vertical = 10.dp).testTag("source-${choice.source.label}"),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(choice.source.label, color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 16.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                parts.joinToString(" · "),
                color = if (failed) colors.danger else colors.secondary,
                fontFamily = ZeronType.Sans, fontSize = 13.sp, maxLines = 2, overflow = TextOverflow.Ellipsis,
            )
        }
        if (choice.current) Text("✓", color = colors.accent, fontSize = 16.sp, modifier = Modifier.padding(start = 8.dp))
    }
}

@Composable
private fun Title(colors: ZeronColors, text: String) {
    Text(text, color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
}

@Composable
internal fun ProgressBar(colors: ZeronColors, p: Float) {
    Box(Modifier.fillMaxWidth().height(6.dp).clip(RoundedCornerShape(3.dp)).background(colors.controlFill)) {
        Box(Modifier.fillMaxWidth(p.coerceIn(0f, 1f)).height(6.dp).clip(RoundedCornerShape(3.dp)).background(colors.accent))
    }
}

/** Half-width sheet / screen button; [danger] tints the label for a destructive action. */
@Composable
internal fun SheetButton(colors: ZeronColors, label: String, modifier: Modifier = Modifier, danger: Boolean = false, onClick: () -> Unit) {
    Box(
        modifier.clip(RoundedCornerShape(24.dp)).background(colors.controlFill).clickable(onClick = onClick).padding(vertical = 13.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = if (danger) colors.danger else colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 15.sp, maxLines = 1)
    }
}
