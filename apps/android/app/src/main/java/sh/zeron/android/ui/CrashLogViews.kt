package sh.zeron.android.ui

import android.content.Context
import android.content.Intent
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.zeron.android.R
import sh.zeron.android.core.CrashLog
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.BackButton
import sh.zeron.android.design.consumeBlankTaps
import sh.zeron.android.design.LocalZeronColors
import sh.zeron.android.design.ZeronType
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/** The Android share sheet with the log as plain text (no upload of our own). */
internal fun shareCrashLog(context: Context, text: String) {
    val send = Intent(Intent.ACTION_SEND)
        .setType("text/plain")
        .putExtra(Intent.EXTRA_SUBJECT, context.getString(R.string.crash_share_subject))
        .putExtra(Intent.EXTRA_TEXT, text)
    runCatching { context.startActivity(Intent.createChooser(send, context.getString(R.string.crash_share))) }
}

/** The stack frames under the headline, for the collapsed preview. */
private fun traceOf(entry: CrashLog.Entry): String {
    val trace = entry.text.substringAfter(CrashLog.TRACE_MARK + "\n", entry.text)
    val lines = trace.lines()
    val frames = if (lines.firstOrNull()?.trim() == entry.headline) lines.drop(1) else lines
    return frames.joinToString("\n") { it.replace("\t", "  ") }
}

private fun crashTime(atMs: Long): String = SimpleDateFormat("yyyy-MM-dd HH:mm:ss", Locale.getDefault()).format(Date(atMs))

/** Next launch after a crash: 上次意外退出 (Zeron Quit Unexpectedly) with 复制日志 / 分享 / 关闭 (Copy Log / Share / Close). Any choice marks it seen. */
@Composable
internal fun LastCrashDialog(model: ZeronModel, entry: CrashLog.Entry) {
    val colors = LocalZeronColors.current
    val context = LocalContext.current
    AlertDialog(
        onDismissRequest = { model.dismissLastCrash() },
        title = { Text(stringResource(R.string.crash_last_title)) },
        text = {
            Column {
                Text(stringResource(R.string.crash_last_body), fontFamily = ZeronType.Sans, fontSize = 14.sp)
                Spacer(Modifier.height(10.dp))
                Text(
                    listOf(crashTime(entry.atMs), entry.headline).filter { it.isNotBlank() }.joinToString("\n"),
                    color = colors.secondary,
                    fontFamily = ZeronType.Mono,
                    fontSize = 11.sp,
                    maxLines = 4,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(colors.controlFill).padding(10.dp),
                )
            }
        },
        confirmButton = {
            Row {
                TextButton(onClick = { model.copyCrashLog(entry.text); model.dismissLastCrash() }, modifier = Modifier.testTag("crash-copy")) {
                    Text(stringResource(R.string.crash_copy_log))
                }
                TextButton(onClick = { shareCrashLog(context, entry.text); model.dismissLastCrash() }, modifier = Modifier.testTag("crash-share")) {
                    Text(stringResource(R.string.crash_share))
                }
            }
        },
        dismissButton = {
            TextButton(onClick = { model.dismissLastCrash() }, modifier = Modifier.testTag("crash-close")) { Text(stringResource(R.string.close)) }
        },
    )
}

/** Settings > About > Crash logs: the saved logs, newest first, each with copy / share; clear all. */
@Composable
internal fun CrashLogsScreen(model: ZeronModel) {
    val colors = LocalZeronColors.current
    val context = LocalContext.current
    var confirmClear by remember { mutableStateOf(false) }
    val logs = model.crashLogs
    Column(Modifier.fillMaxSize().background(colors.background).consumeBlankTaps().statusBarsPadding().navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            BackButton(colors, onClick = { model.showCrashLogs = false })
            Text(stringResource(R.string.crash_logs), color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp, modifier = Modifier.weight(1f), textAlign = TextAlign.Center)
            Text(
                stringResource(R.string.crash_logs_clear),
                color = if (logs.isEmpty()) colors.tertiary else colors.danger,
                fontFamily = ZeronType.Sans,
                fontWeight = FontWeight.SemiBold,
                fontSize = 16.sp,
                modifier = Modifier.clip(RoundedCornerShape(12.dp)).clickable(enabled = logs.isNotEmpty()) { confirmClear = true }.padding(8.dp),
            )
        }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp)) {
            Text(stringResource(R.string.crash_logs_note), color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp, modifier = Modifier.padding(start = 4.dp, end = 4.dp, top = 6.dp, bottom = 10.dp))
            if (logs.isEmpty()) SettingRow(colors, stringResource(R.string.crash_logs_empty), null)
            for (entry in logs) {
                var expanded by rememberSaveable(entry.file.name) { mutableStateOf(false) }
                Column(
                    Modifier.fillMaxWidth().padding(vertical = 4.dp).clip(RoundedCornerShape(14.dp)).background(colors.elevated)
                        .clickable { expanded = !expanded }.padding(horizontal = 14.dp, vertical = 12.dp),
                ) {
                    Text(crashTime(entry.atMs), color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 16.sp)
                    if (entry.headline.isNotBlank()) {
                        Text(entry.headline, color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    }
                    Spacer(Modifier.height(8.dp))
                    Text(
                        if (expanded) entry.text.replace("\t", "  ") else traceOf(entry),
                        color = colors.text,
                        fontFamily = ZeronType.Mono,
                        fontSize = 11.sp,
                        lineHeight = 15.sp,
                        maxLines = if (expanded) Int.MAX_VALUE else 6,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Spacer(Modifier.height(10.dp))
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Pill(colors, stringResource(R.string.copy)) { model.copyCrashLog(entry.text) }
                        Pill(colors, stringResource(R.string.crash_share)) { shareCrashLog(context, entry.text) }
                    }
                }
            }
            Spacer(Modifier.height(40.dp))
        }
    }
    if (confirmClear) {
        AlertDialog(
            onDismissRequest = { confirmClear = false },
            title = { Text(stringResource(R.string.crash_logs_clear_confirm)) },
            text = { Text(stringResource(R.string.crash_logs_note)) },
            confirmButton = { TextButton(onClick = { confirmClear = false; model.clearCrashLogs() }) { Text(stringResource(R.string.crash_logs_clear)) } },
            dismissButton = { TextButton(onClick = { confirmClear = false }) { Text(stringResource(R.string.cancel)) } },
        )
    }
}
