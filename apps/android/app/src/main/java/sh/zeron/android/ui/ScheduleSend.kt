package sh.zeron.android.ui

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
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
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Text
import androidx.compose.material3.TimePicker
import androidx.compose.material3.TimePickerDefaults
import androidx.compose.material3.rememberTimePickerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.testTag
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import sh.zeron.android.R
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.Glyphs
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronType
import sh.zeron.android.design.glassSurface
import sh.zeron.android.schedule.ScheduleTime
import sh.zeron.android.schedule.ScheduledAlarms
import sh.zeron.android.schedule.ScheduledMessage
import java.util.Calendar

/** The two ways to pick the moment: a clock time, or a duration from now. */
internal enum class ScheduleMode { AT, AFTER }

/** "After" quick picks, in minutes. */
private val QuickMinutes = listOf(5, 15, 30, 60, 120)

/** When [atMs] is relative to now: "13:20", "明天 13:20" / "tomorrow 13:20", or "10月2日 13:20" (a date: Oct 2, 13:20). */
internal fun scheduleWhenText(context: android.content.Context, atMs: Long, nowMs: Long = System.currentTimeMillis()): String {
    val clock = ScheduleTime.clock(atMs)
    return when (ScheduleTime.daysFrom(atMs, nowMs)) {
        0 -> clock
        1 -> context.getString(R.string.schedule_when_tomorrow, clock)
        else -> context.getString(R.string.schedule_when_date, scheduleDateText(context, atMs), clock)
    }
}

@Composable
internal fun scheduleWhen(atMs: Long): String {
    androidx.compose.ui.platform.LocalConfiguration.current // recompose on a language switch
    return scheduleWhenText(LocalContext.current, atMs)
}

private fun scheduleDateText(context: android.content.Context, atMs: Long): String {
    val locale = context.resources.configuration.locales[0]
    val pattern = android.text.format.DateFormat.getBestDateTimePattern(locale, "MMMd")
    return java.text.SimpleDateFormat(pattern, locale).format(java.util.Date(atMs))
}

/** The dialog's line: "Today 13:20", "Tomorrow 13:20" or "Oct 2 13:20". */
@Composable
private fun scheduleDayLabel(atMs: Long, nowMs: Long): String {
    val clock = ScheduleTime.clock(atMs)
    return when (ScheduleTime.daysFrom(atMs, nowMs)) {
        0 -> stringResource(R.string.schedule_today, clock)
        1 -> stringResource(R.string.schedule_tomorrow, clock)
        else -> stringResource(R.string.schedule_when_date, scheduleDateText(LocalContext.current, atMs), clock)
    }
}

@Composable
private fun durationLabel(minutes: Int): String =
    if (minutes % 60 == 0) stringResource(R.string.schedule_hours_short, minutes / 60) else stringResource(R.string.schedule_minutes_short, minutes)

/**
 * Long-press Send → Schedule send. Two tabs: "At time" picks a clock time
 * (today, or tomorrow when that has passed); "After" picks a duration
 * (quick chips or hours + minutes) and schedules now + duration. Either
 * way [onSchedule] gets an absolute phone wall-clock time.
 */
@OptIn(ExperimentalMaterial3Api::class, androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
internal fun ScheduleSendDialog(
    colors: ZeronColors,
    initialMode: ScheduleMode = ScheduleMode.AT,
    onDismiss: () -> Unit,
    onSchedule: (atMs: Long) -> Unit,
) {
    val context = LocalContext.current
    var mode by rememberSaveable { mutableStateOf(initialMode) }
    val start = remember {
        // Default: five minutes out, on a five-minute mark.
        Calendar.getInstance().apply {
            add(Calendar.MINUTE, 5 + (5 - get(Calendar.MINUTE) % 5) % 5)
        }
    }
    val state = rememberTimePickerState(
        initialHour = start.get(Calendar.HOUR_OF_DAY),
        initialMinute = start.get(Calendar.MINUTE) / 5 * 5,
        is24Hour = android.text.format.DateFormat.is24HourFormat(context),
    )
    // "After": the duration as text, so the fields can be empty while typing.
    var hoursText by rememberSaveable { mutableStateOf("0") }
    var minutesText by rememberSaveable { mutableStateOf("30") }
    val afterMinutes = (hoursText.toIntOrNull() ?: 0) * 60 + (minutesText.toIntOrNull() ?: 0)
    // Re-read the clock every few seconds so "After" stays now + duration.
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) { while (true) { kotlinx.coroutines.delay(5_000); now = System.currentTimeMillis() } }
    val at = when (mode) {
        ScheduleMode.AT -> ScheduleTime.next(state.hour, state.minute, now)
        ScheduleMode.AFTER -> ScheduleTime.after(afterMinutes, now)
    }
    val canConfirm = mode == ScheduleMode.AT || afterMinutes > 0
    val exact = remember { ScheduledAlarms.canExact(context) }
    val well = if (colors.dark) Color(0xFF2C2C2E) else Color(0xFFEFEFF1)
    Dialog(onDismissRequest = onDismiss) {
        Column(
            Modifier
                .widthIn(max = 400.dp)
                .clip(RoundedCornerShape(28.dp))
                .background(colors.sheet)
                .padding(horizontal = 20.dp, vertical = 18.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(
                stringResource(R.string.schedule_send_title),
                color = colors.text,
                fontFamily = ZeronType.Sans,
                fontWeight = FontWeight.SemiBold,
                fontSize = 17.sp,
                modifier = Modifier.fillMaxWidth(),
            )
            Spacer(Modifier.height(14.dp))
            Segmented(
                colors,
                well,
                listOf(stringResource(R.string.schedule_tab_at), stringResource(R.string.schedule_tab_after)),
                selected = mode.ordinal,
            ) { mode = ScheduleMode.entries[it] }
            Spacer(Modifier.height(16.dp))
            when (mode) {
                ScheduleMode.AT -> TimePicker(
                    state = state,
                    colors = TimePickerDefaults.colors(
                        clockDialColor = well,
                        selectorColor = colors.accent,
                        timeSelectorSelectedContainerColor = colors.accent.copy(alpha = 0.22f),
                        timeSelectorUnselectedContainerColor = well,
                        timeSelectorSelectedContentColor = colors.text,
                        timeSelectorUnselectedContentColor = colors.secondary,
                        periodSelectorSelectedContainerColor = colors.accent.copy(alpha = 0.22f),
                        periodSelectorUnselectedContainerColor = Color.Transparent,
                        periodSelectorSelectedContentColor = colors.text,
                        periodSelectorUnselectedContentColor = colors.secondary,
                        clockDialSelectedContentColor = Color.White,
                        clockDialUnselectedContentColor = colors.secondary,
                    ),
                )
                ScheduleMode.AFTER -> Column(Modifier.fillMaxWidth()) {
                    androidx.compose.foundation.layout.FlowRow(
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        QuickMinutes.forEach { m ->
                            val on = afterMinutes == m
                            Text(
                                durationLabel(m),
                                color = if (on) colors.text else colors.secondary,
                                fontFamily = ZeronType.Sans,
                                fontWeight = FontWeight.Medium,
                                fontSize = 14.sp,
                                modifier = Modifier
                                    .clip(RoundedCornerShape(16.dp))
                                    .background(if (on) colors.accent.copy(alpha = 0.22f) else well)
                                    .clickable {
                                        hoursText = (m / 60).toString()
                                        minutesText = (m % 60).toString()
                                    }
                                    .padding(horizontal = 14.dp, vertical = 8.dp),
                            )
                        }
                    }
                    Spacer(Modifier.height(18.dp))
                    Text(stringResource(R.string.schedule_custom), color = colors.secondary, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 13.sp)
                    Spacer(Modifier.height(8.dp))
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        NumberField(colors, well, hoursText, max = 99, tag = "schedule-hours") { hoursText = it }
                        Spacer(Modifier.width(8.dp))
                        Text(stringResource(R.string.schedule_hours_unit), color = colors.text, fontFamily = ZeronType.Sans, fontSize = 15.sp)
                        Spacer(Modifier.width(18.dp))
                        NumberField(colors, well, minutesText, max = 59, tag = "schedule-minutes") { minutesText = it }
                        Spacer(Modifier.width(8.dp))
                        Text(stringResource(R.string.schedule_minutes_unit), color = colors.text, fontFamily = ZeronType.Sans, fontSize = 15.sp)
                    }
                    Spacer(Modifier.height(18.dp))
                }
            }
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Glyph(Glyphs.Clock, 15.dp, colors.secondary)
                Spacer(Modifier.width(6.dp))
                Text(
                    if (canConfirm) scheduleDayLabel(at, now) else "—",
                    color = colors.text,
                    fontFamily = ZeronType.Sans,
                    fontWeight = FontWeight.Medium,
                    fontSize = 15.sp,
                )
            }
            if (!exact) {
                Spacer(Modifier.height(10.dp))
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        stringResource(R.string.schedule_exact_hint),
                        color = colors.tertiary,
                        fontFamily = ZeronType.Sans,
                        fontSize = 12.5.sp,
                        modifier = Modifier.weight(1f),
                    )
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                        Text(
                            stringResource(R.string.schedule_exact_allow),
                            color = colors.accent,
                            fontFamily = ZeronType.Sans,
                            fontWeight = FontWeight.Medium,
                            fontSize = 13.sp,
                            modifier = Modifier.clip(RoundedCornerShape(8.dp)).clickable {
                                runCatching {
                                    context.startActivity(
                                        Intent(Settings.ACTION_REQUEST_SCHEDULE_EXACT_ALARM, Uri.parse("package:${context.packageName}"))
                                            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                                    )
                                }
                            }.padding(horizontal = 8.dp, vertical = 6.dp),
                        )
                    }
                }
            }
            Spacer(Modifier.height(16.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End, verticalAlignment = Alignment.CenterVertically) {
                Text(
                    stringResource(R.string.schedule_dismiss),
                    color = colors.secondary,
                    fontFamily = ZeronType.Sans,
                    fontWeight = FontWeight.Medium,
                    fontSize = 15.sp,
                    modifier = Modifier.clip(RoundedCornerShape(18.dp)).clickable(onClick = onDismiss).padding(horizontal = 14.dp, vertical = 9.dp),
                )
                Spacer(Modifier.width(6.dp))
                Text(
                    stringResource(R.string.schedule_confirm),
                    color = Color.White.copy(alpha = if (canConfirm) 1f else 0.5f),
                    fontFamily = ZeronType.Sans,
                    fontWeight = FontWeight.SemiBold,
                    fontSize = 15.sp,
                    modifier = Modifier
                        .clip(RoundedCornerShape(18.dp))
                        .background(colors.accent.copy(alpha = if (canConfirm) 1f else 0.4f))
                        .clickable(enabled = canConfirm) {
                            val fire = System.currentTimeMillis()
                            onSchedule(
                                when (mode) {
                                    ScheduleMode.AT -> ScheduleTime.next(state.hour, state.minute, fire)
                                    ScheduleMode.AFTER -> ScheduleTime.after(afterMinutes, fire)
                                },
                            )
                        }
                        .padding(horizontal = 16.dp, vertical = 9.dp),
                )
            }
        }
    }
}

/** Two-segment control (iOS UISegmentedControl look). */
@Composable
private fun Segmented(colors: ZeronColors, well: Color, labels: List<String>, selected: Int, onSelect: (Int) -> Unit) {
    Row(Modifier.fillMaxWidth().height(34.dp).clip(RoundedCornerShape(10.dp)).background(well).padding(2.dp)) {
        labels.forEachIndexed { i, label ->
            val on = i == selected
            Box(
                Modifier
                    .weight(1f)
                    .fillMaxHeight()
                    .clip(RoundedCornerShape(8.dp))
                    .background(if (on) (if (colors.dark) Color(0xFF636366) else Color.White) else Color.Transparent)
                    .clickable { onSelect(i) }
                    .testTag("schedule-tab-$i"),
                contentAlignment = Alignment.Center,
            ) {
                Text(label, color = colors.text, fontFamily = ZeronType.Sans, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Medium, fontSize = 14.sp, maxLines = 1)
            }
        }
    }
}

/** Small digits-only field (0…[max]). */
@Composable
private fun NumberField(colors: ZeronColors, well: Color, value: String, max: Int, tag: String, onValue: (String) -> Unit) {
    androidx.compose.foundation.text.BasicTextField(
        value = value,
        onValueChange = { raw ->
            val digits = raw.filter { it.isDigit() }.take(2)
            onValue(if (digits.isEmpty()) "" else minOf(digits.toInt(), max).toString())
        },
        singleLine = true,
        keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(keyboardType = androidx.compose.ui.text.input.KeyboardType.Number),
        textStyle = androidx.compose.ui.text.TextStyle(color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 17.sp, textAlign = androidx.compose.ui.text.style.TextAlign.Center),
        cursorBrush = androidx.compose.ui.graphics.SolidColor(colors.accent),
        modifier = Modifier.width(60.dp).height(40.dp).clip(RoundedCornerShape(10.dp)).background(well).testTag(tag),
        decorationBox = { inner -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { inner() } },
    )
}

/** Above the composer: "Scheduled for 01:20 · Cancel" (iOS status-pill geometry); [detail] after the time. */
@Composable
internal fun ScheduledChip(colors: ZeronColors, message: ScheduledMessage, detail: String? = null, onCancel: () -> Unit) {
    Row(Modifier.padding(bottom = 8.dp)) {
        Row(
            Modifier.height(30.dp).glassSurface(colors, 15.dp).padding(start = 10.dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Glyph(Glyphs.Clock, 14.dp, colors.accent)
            Spacer(Modifier.width(7.dp))
            Text(
                stringResource(R.string.schedule_chip, scheduleWhen(message.atMs)) + (detail?.takeIf { it.isNotBlank() }?.let { " · $it" } ?: ""),
                color = colors.secondary,
                fontFamily = ZeronType.Sans,
                fontWeight = FontWeight.Medium,
                fontSize = 13.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f, fill = false),
            )
            Text(" · ", color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = 13.sp)
            Box(Modifier.clip(RoundedCornerShape(11.dp)).clickable(onClick = onCancel).padding(horizontal = 6.dp, vertical = 4.dp)) {
                Text(stringResource(R.string.schedule_chip_cancel), color = colors.accent, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 13.sp)
            }
        }
    }
}

/** Scheduled new sessions on this workspace (they have no chat yet to show a chip in). */
@Composable
internal fun rememberScheduledNewSessions(workspace: String): List<ScheduledMessage> {
    val context = LocalContext.current
    val tick by sh.zeron.android.schedule.ScheduledStore.changes.collectAsState()
    return remember(tick, workspace) {
        sh.zeron.android.schedule.ScheduledStore(context).list().filter { it.newSession != null && it.workspace == workspace }
    }
}

/**
 * Home list entry per scheduled new session: "Scheduled new session ·
 * 13:20", the message's first line and project, and Cancel.
 */
@Composable
internal fun ScheduledNewSessionRows(colors: ZeronColors, messages: List<ScheduledMessage>, onCancel: (ScheduledMessage) -> Unit) {
    messages.forEach { message ->
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp).clip(RoundedCornerShape(14.dp)).background(colors.elevated)
                .padding(start = 14.dp, end = 8.dp, top = 10.dp, bottom = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Glyph(Glyphs.Clock, 16.dp, colors.accent)
            Spacer(Modifier.width(10.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    stringResource(R.string.sched_new_home_title, scheduleWhen(message.atMs)),
                    color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
                val first = message.text.lineSequence().firstOrNull().orEmpty()
                val label = message.newSession?.label.orEmpty()
                Text(
                    listOf(first, label).filter { it.isNotBlank() }.joinToString(" · "),
                    color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
            }
            Spacer(Modifier.width(8.dp))
            Text(
                stringResource(R.string.schedule_chip_cancel),
                color = colors.accent, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 14.sp,
                modifier = Modifier.clip(RoundedCornerShape(10.dp)).clickable { onCancel(message) }.padding(horizontal = 8.dp, vertical = 6.dp),
            )
        }
    }
}
