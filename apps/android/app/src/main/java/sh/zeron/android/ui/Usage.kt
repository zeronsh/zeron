package sh.zeron.android.ui

import androidx.compose.foundation.layout.width
import sh.zeron.android.design.BackButton
import sh.zeron.android.R
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronType
import uniffi.zeron_core.AgentUsage
import uniffi.zeron_core.ContextUsage
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * Model usage for one session, the two things the engine reports:
 *
 * - **Context window**: tokens in the conversation vs. the model's window,
 *   from the session doc (`contextUsage`, the same number behind iOS's
 *   "N% context" chip). Claude Code, Codex, OpenCode and the ACP agents
 *   (Devin, Grok, Hermes, Pi, Antigravity) report it.
 * - **Plan usage**: the rate-limit windows of the host's signed-in account
 *   for this harness (host `ListAgentAccounts`, what the desktop's plan ring
 *   shows). Works over the relay and the direct SSH link alike.
 *
 * Per-turn token counts and cost aren't stored by the engine, so there is
 * nothing to show for them.
 */
@Composable
internal fun UsageSheet(
    colors: ZeronColors,
    loadUsage: suspend (deviceId: String, force: Boolean) -> List<AgentUsage>,
    deviceId: String,
    harness: String?,
    context: ContextUsage?,
    onAccounts: (List<AgentUsage>) -> Unit = {},
    onClose: () -> Unit,
) {
    BackHandler(onBack = onClose)
    val harnessName = harness?.let { runCatching { uniffi.zeron_core.harnessLabel(it) }.getOrNull() } ?: stringResource(R.string.the_agent)
    var accounts by remember { mutableStateOf<List<AgentUsage>?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }
    var refreshing by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(deviceId) {
        // Cached probe first (instant), then a forced one so it's current.
        runCatching { loadUsage(deviceId, false) }
            .onSuccess { accounts = it; onAccounts(it) }
            .onFailure { failure = it.message }
        refreshing = true
        runCatching { loadUsage(deviceId, true) }
            .onSuccess { accounts = it; failure = null; onAccounts(it) }
        refreshing = false
    }
    val none = remember { MutableInteractionSource() }
    Box(
        Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.45f)).clickable(interactionSource = none, indication = null, onClick = onClose),
        contentAlignment = Alignment.BottomCenter,
    ) {
        Column(
            Modifier
                .widthIn(max = 560.dp)
                .fillMaxWidth()
                .windowInsetsPadding(WindowInsets.ime.union(WindowInsets.navigationBars))
                .padding(12.dp)
                // Solid (not glass): meters over moving transcript text read poorly.
                .clip(RoundedCornerShape(28.dp))
                .background(colors.sheet)
                .clickable(interactionSource = none, indication = null) {}
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 20.dp, vertical = 18.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                BackButton(colors, onClick = onClose)
                Spacer(Modifier.width(10.dp))
                Text(stringResource(R.string.usage), color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp, modifier = Modifier.weight(1f))
            }
            Spacer(Modifier.height(14.dp))
            SectionTitle(colors, stringResource(R.string.usage_context_window))
            val tokens = context?.tokens?.toLong()
            val window = context?.window?.toLong()?.takeIf { it > 0 }
            when {
                tokens != null && window != null -> {
                    val f = (tokens.toDouble() / window).coerceIn(0.0, 1.0).toFloat()
                    Meter(colors, stringResource(R.string.usage_percent_used, Math.round(f * 100)), stringResource(R.string.usage_tokens_of, compact(tokens), compact(window)), f)
                }
                tokens != null -> Line(colors, stringResource(R.string.usage_tokens_in_context, compact(tokens)))
                else -> Line(colors, stringResource(R.string.usage_no_context, harnessName))
            }
            Spacer(Modifier.height(18.dp))
            val mine = accounts?.filter { harness == null || it.harness == harness }.orEmpty().sortedByDescending { it.active }
            Row(verticalAlignment = Alignment.CenterVertically) {
                SectionTitle(colors, stringResource(R.string.usage_plan, harnessName), Modifier.weight(1f))
                Text(
                    stringResource(if (refreshing) R.string.updating else R.string.refresh),
                    color = if (refreshing) colors.tertiary else colors.accent,
                    fontFamily = ZeronType.Sans,
                    fontSize = 13.sp,
                    modifier = Modifier.clip(RoundedCornerShape(8.dp)).clickable(enabled = !refreshing) {
                        scope.launch {
                            refreshing = true
                            runCatching { loadUsage(deviceId, true) }
                                .onSuccess { accounts = it; failure = null; onAccounts(it) }
                                .onFailure { failure = it.message }
                            refreshing = false
                        }
                    }.padding(horizontal = 6.dp, vertical = 4.dp),
                )
            }
            when {
                accounts == null && failure == null -> Line(colors, stringResource(R.string.loading))
                accounts == null -> Line(colors, if (failure?.contains("Unsupported", true) == true || failure?.contains("unknown", true) == true) stringResource(R.string.usage_unsupported) else stringResource(R.string.usage_load_failed, failure.orEmpty()))
                mine.isEmpty() -> Line(colors, stringResource(R.string.usage_no_login, harnessName))
                else -> mine.forEachIndexed { i, account ->
                    if (i > 0) Spacer(Modifier.height(14.dp))
                    AccountUsage(colors, account)
                }
            }
        }
    }
}

@Composable
private fun AccountUsage(colors: ZeronColors, a: AgentUsage) {
    val res = LocalContext.current.resources
    val locale = res.configuration.locales[0] ?: Locale.getDefault()
    val who = listOfNotNull(a.planLabel, a.email).joinToString(" · ").ifEmpty { stringResource(R.string.signed_in) }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(who, color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 14.sp, modifier = Modifier.weight(1f, fill = false))
        if (a.active) {
            Spacer(Modifier.padding(start = 8.dp))
            Text(stringResource(R.string.active), color = colors.success, fontFamily = ZeronType.Sans, fontSize = 12.sp, modifier = Modifier.clip(RoundedCornerShape(6.dp)).background(colors.success.copy(alpha = 0.12f)).padding(horizontal = 6.dp, vertical = 2.dp))
        }
    }
    Spacer(Modifier.height(8.dp))
    if (a.windows.isEmpty() && a.error == null) Line(colors, stringResource(R.string.usage_none_yet))
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        a.windows.forEach { w ->
            Meter(colors, "${w.label} · ${Math.round(w.usedFraction * 100)}%", w.resetsAtMs?.let { resets(it, res, locale) } ?: "", w.usedFraction, planTone(colors, w.usedFraction))
        }
    }
    a.error?.let {
        Spacer(Modifier.height(6.dp))
        Text(it, color = colors.warning, fontFamily = ZeronType.Sans, fontSize = 12.5.sp)
    }
    a.fetchedAtMs?.let {
        Spacer(Modifier.height(6.dp))
        Text(stringResource(R.string.usage_updated, ago(it, res)), color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = 12.sp)
    }
}

@Composable
private fun SectionTitle(colors: ZeronColors, text: String, modifier: Modifier = Modifier) {
    Text(text, color = colors.secondary, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 13.sp, modifier = modifier.padding(bottom = 8.dp))
}

@Composable
private fun Line(colors: ZeronColors, text: String) {
    Text(text, color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = 14.sp)
}

/**
 * A labelled bar. Context steps up like the desktop context ring (75% /
 * 90%); plan windows pass the desktop meter tone (80% / 95%).
 */
@Composable
private fun Meter(
    colors: ZeronColors,
    label: String,
    detail: String,
    fraction: Float,
    tone: Color = when {
        fraction >= 0.9f -> colors.danger
        fraction >= 0.75f -> colors.warning
        else -> colors.accent
    },
) {
    Column {
        Row {
            Text(label, color = colors.text, fontFamily = ZeronType.Sans, fontSize = 14.sp, modifier = Modifier.weight(1f))
            Text(detail, color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = 13.sp)
        }
        Spacer(Modifier.height(6.dp))
        Box(Modifier.fillMaxWidth().height(6.dp).clip(RoundedCornerShape(3.dp)).background(colors.controlFill)) {
            Box(Modifier.fillMaxWidth(fraction.coerceIn(0f, 1f)).height(6.dp).clip(RoundedCornerShape(3.dp)).background(tone))
        }
    }
}

internal fun compact(n: Long): String = when {
    n >= 1_000_000 -> String.format(Locale.US, "%.1fM", n / 1_000_000.0).replace(".0M", "M")
    n >= 10_000 -> "${n / 1000}k"
    n >= 1_000 -> String.format(Locale.US, "%.1fk", n / 1000.0).replace(".0k", "k")
    else -> "$n"
}

/** "resets 3:05 PM" / "15:05 重置": the time, weekday or date in the app's locale. */
private fun resets(ms: Long, res: android.content.res.Resources, locale: Locale): String {
    val left = ms - System.currentTimeMillis()
    val skeleton = when {
        left < 22 * 3_600_000L -> "jmm"
        left < 7 * 86_400_000L -> "EEE"
        else -> "MMMd"
    }
    val pattern = android.text.format.DateFormat.getBestDateTimePattern(locale, skeleton)
    // ICU's formatter: best patterns can use standalone fields (ccc, LLL).
    return res.getString(R.string.usage_resets, android.icu.text.SimpleDateFormat(pattern, locale).format(Date(ms)))
}

private fun ago(ms: Long, res: android.content.res.Resources): String {
    val s = ((System.currentTimeMillis() - ms) / 1000).coerceAtLeast(0)
    return when {
        s < 60 -> res.getString(R.string.ago_just_now)
        s < 3600 -> res.getString(R.string.ago_minutes, (s / 60).toInt())
        s < 86_400 -> res.getString(R.string.ago_hours, (s / 3600).toInt())
        else -> res.getString(R.string.ago_days, (s / 86_400).toInt())
    }
}
