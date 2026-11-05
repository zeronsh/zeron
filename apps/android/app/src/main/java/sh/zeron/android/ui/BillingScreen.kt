package sh.zeron.android.ui

import android.view.HapticFeedbackConstants
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.IconButton
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.CloudStore
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.BillingInfo
import uniffi.zeron_core.BillingPlanInfo
import uniffi.zeron_core.CoreClient
import kotlin.math.max
import kotlin.math.roundToLong

private const val MAX_PACKS = 20

/** "about 750 hours" of the default machine. */
private fun hours(credits: Double) = "about ${CloudStore.credits((credits / 60).roundToLong().toDouble())} hours"

/**
 * Settings → Billing (iOS `BillingViewController`): the balance and the time
 * it buys, the monthly plans, and extra credits that never expire. Payment
 * pages open in the browser; the page re-reads when the app comes back.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun BillingScreen(model: AppModel, onBack: (() -> Unit)?) {
    val cloud = model.cloud
    val context = LocalContext.current
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    var packs by remember { mutableIntStateOf(1) }
    var busy by remember { mutableStateOf<String?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) { cloud.refreshBilling() }

    /** Back from Stripe: Checkout lands through a webhook, so re-read for a little while until the change shows. */
    suspend fun settle() {
        val before = cloud.billing?.plan?.id to cloud.billing?.creditsTotal
        repeat(6) {
            cloud.refreshBilling()
            if ((cloud.billing?.plan?.id to cloud.billing?.creditsTotal) != before) return
            delay(2_000)
        }
    }

    fun open(key: String, link: suspend (CoreClient) -> String) {
        val c = model.client.value ?: return
        if (busy != null) return
        busy = key
        scope.launch {
            try {
                val url = link(c)
                model.browser.open(context, url)
                // The Custom Tab is up: wait for the app to come back, then catch the change.
                launch {
                    model.browser.resumes.first()
                    settle()
                }
            } catch (e: Exception) {
                failure = CloudStore.message(e)
            } finally {
                busy = null
            }
        }
    }

    CloudPage("Billing", cloud.billing?.plan?.let { "${it.name} plan" }, onBack) {
        val billing = cloud.billing
        if (billing == null) {
            val error = cloud.billingError
            if (error != null) {
                // A failed read is said, with a way forward — never a spinner that runs forever.
                item("error") {
                    Column(Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 32.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                        Text("Couldn't load billing. $error", style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        Spacer(Modifier.height(20.dp))
                        SecondaryCapsule("Try again", onClick = { scope.launch { cloud.refreshBilling() } }, icon = ZIcons.Refresh)
                    }
                }
            } else {
                loadingItem()
            }
            return@CloudPage
        }
        if (!billing.available) {
            item("unavailable") {
                Text(
                    "Billing isn't set up on this server yet, so plans and credits can't be bought here.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 28.dp, vertical = 8.dp),
                )
            }
        }
        cloudSection("Credits") { Credits(billing) }
        cloudSection("Plan") {
            val current = billing.plan?.id
            val idle = busy == null && billing.available
            billing.plans.forEachIndexed { ix, plan ->
                PlanCard(
                    plan,
                    current = plan.id == current,
                    busy = busy == "plan:${plan.id}",
                    action = when {
                        plan.id == current -> null
                        current != null -> "Switch"
                        else -> "Choose ${plan.name}"
                    },
                    solid = current == null && ix == 0,
                    enabled = idle,
                ) {
                    view.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
                    // On a plan, switching goes through Stripe's portal.
                    if (current != null) open("plan:${plan.id}") { it.billingPortal() }
                    else open("plan:${plan.id}") { it.billingCheckout("subscription", plan.id, null) }
                }
                if (ix < billing.plans.size - 1) Spacer(Modifier.height(6.dp))
            }
            billing.plan?.let { plan ->
                Spacer(Modifier.height(6.dp))
                val (detail, warn) = when {
                    billing.planStatus == "past_due" -> "Your last payment didn't go through — update your card to keep your plan." to true
                    billing.cancelAtPeriodEnd -> (billing.periodEndMs?.let { "Ends ${CloudStore.shortDate(it)}. You keep its credits until then." } ?: "Won't renew.") to false
                    else -> (billing.periodEndMs?.let { "Renews ${CloudStore.shortDate(it)}." } ?: "Renews monthly.") to false
                }
                Surface(shape = RoundedCornerShape(24.dp), color = cardColor(), modifier = Modifier.fillMaxWidth()) {
                    Row(Modifier.padding(horizontal = 20.dp, vertical = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text("${plan.name} · ${CloudStore.price(plan.priceCents)} a month", style = MaterialTheme.typography.titleMedium)
                            Text(
                                detail,
                                style = MaterialTheme.typography.bodyMedium,
                                color = if (warn) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                        Spacer(Modifier.width(12.dp))
                        OutlinedButton(onClick = { open("portal") { it.billingPortal() } }, enabled = busy == null) {
                            Text(if (busy == "portal") "Opening…" else "Manage")
                        }
                    }
                }
            }
        }
        cloudSection("Extra credits", footer = "Packs never expire and are used once plan credits run out.") {
            ExtraCredits(
                billing,
                packs,
                onPacks = {
                    view.performHapticFeedback(HapticFeedbackConstants.CLOCK_TICK)
                    packs = it
                },
                busy = busy == "credits",
                enabled = busy == null && billing.available,
            ) { open("credits") { it.billingCheckout("credits", null, packs.toUInt()) } }
        }
    }

    failure?.let { message ->
        AlertDialog(
            onDismissRequest = { failure = null },
            icon = { ZIcon(ZIcons.Wallet, null) },
            title = { Text("Couldn't open payments") },
            text = { Text(message) },
            confirmButton = { TextButton(onClick = { failure = null }) { Text("OK") } },
        )
    }
}

/** The balance: one number and the time it buys, a bar split into plan and extra credits, then a row for each kind. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun Credits(b: BillingInfo) {
    val total = max(b.creditsTotal, 0.0)
    val monthly = max(b.creditsMonthly, 0.0)
    val extra = max(b.creditsExtra, 0.0)
    Surface(shape = segmentedShapes(0, 3).shape, color = cardColor(), modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(20.dp)) {
            Row(verticalAlignment = Alignment.Bottom) {
                Text(
                    CloudStore.credits(total),
                    style = MaterialTheme.typography.displaySmallEmphasized,
                    modifier = Modifier.alignByBaseline().semantics { contentDescription = "${CloudStore.credits(total)} credits" },
                )
                Spacer(Modifier.width(8.dp))
                Text(if (total == 1.0) "credit" else "credits", style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.alignByBaseline())
            }
            Spacer(Modifier.height(4.dp))
            Text(
                if (total > 0) hours(total) else "Choose a plan or buy credits to run Cloud",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (monthly + extra > 0) {
                Spacer(Modifier.height(16.dp))
                SplitBar(listOf(monthly to 1f, extra to 0.45f))
            }
        }
    }
    val plan = if (b.creditsMonthlyTotal > 0) {
        val of = "Of ${CloudStore.credits(b.creditsMonthlyTotal)}"
        (b.creditsMonthlyEndsMs?.let { "$of · resets ${CloudStore.shortDate(it)}" } ?: of) to CloudStore.credits(monthly)
    } else {
        "No plan yet — choose one below" to "—"
    }
    CloudRow(
        1, 3, "Plan credits", plan.first,
        leading = { IconTile(ZIcons.Restart) },
        trailing = { Value(plan.second) },
    )
    CloudRow(
        2, 3, "Extra credits", "Never expire · used once plan credits run out",
        leading = { IconTile(ZIcons.Wallet) },
        trailing = { Value(CloudStore.credits(extra)) },
    )
}

@Composable
private fun Value(text: String) = Text(text, style = MaterialTheme.typography.titleMedium)

/** A thin track split into shares of the primary colour. */
@Composable
private fun SplitBar(parts: List<Pair<Double, Float>>) {
    val total = max(parts.sumOf { it.first }, 1.0)
    val color = MaterialTheme.colorScheme.primary
    Row(Modifier.fillMaxWidth().height(8.dp).clip(CircleShape).background(MaterialTheme.colorScheme.surfaceContainerHighest)) {
        for ((value, alpha) in parts) {
            if (value <= 0) continue
            Box(Modifier.weight((value / total).toFloat()).fillMaxHeight().background(color.copy(alpha = alpha)))
        }
        val used = parts.sumOf { it.first }
        if (used < total) Spacer(Modifier.weight(((total - used) / total).toFloat().coerceAtLeast(0.0001f)))
    }
}

/** A plan: its name (and "Current"), the price per month, the credits it brings and the time they buy, then its one action. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun PlanCard(plan: BillingPlanInfo, current: Boolean, busy: Boolean, action: String?, solid: Boolean, enabled: Boolean, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Surface(
        shape = RoundedCornerShape(24.dp),
        color = if (current) scheme.secondaryContainer else cardColor(),
        border = if (current) BorderStroke(1.5.dp, scheme.primary.copy(alpha = 0.6f)) else null,
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(20.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(plan.name, style = MaterialTheme.typography.titleSmallEmphasized, color = scheme.onSurfaceVariant, modifier = Modifier.weight(1f))
                if (current) {
                    Surface(shape = RoundedCornerShape(8.dp), color = scheme.primary, contentColor = scheme.onPrimary) {
                        Text("Current", style = MaterialTheme.typography.labelMedium, modifier = Modifier.padding(horizontal = 8.dp, vertical = 2.dp))
                    }
                }
            }
            Spacer(Modifier.height(6.dp))
            Row(verticalAlignment = Alignment.Bottom) {
                Text(CloudStore.price(plan.priceCents), style = MaterialTheme.typography.headlineMediumEmphasized, modifier = Modifier.alignByBaseline())
                Spacer(Modifier.width(6.dp))
                Text("/ month", style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant, modifier = Modifier.alignByBaseline())
            }
            Spacer(Modifier.height(6.dp))
            Text("${CloudStore.credits(plan.monthlyCredits)} credits a month", style = MaterialTheme.typography.bodyLarge)
            Text(hours(plan.monthlyCredits), style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
            if (action != null) {
                Spacer(Modifier.height(16.dp))
                val content: @Composable () -> Unit = {
                    if (busy) LoadingIndicator(Modifier.size(24.dp)) else Text(action, style = MaterialTheme.typography.titleSmall)
                }
                if (solid) {
                    Button(onClick = onClick, enabled = enabled, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { content() }
                } else {
                    FilledTonalButton(onClick = onClick, enabled = enabled, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { content() }
                }
            }
        }
    }
}

/** One-off packs: how many (− count +), then Buy. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun ExtraCredits(b: BillingInfo, packs: Int, onPacks: (Int) -> Unit, busy: Boolean, enabled: Boolean, onBuy: () -> Unit) {
    val credits = b.packCredits * packs
    Surface(shape = RoundedCornerShape(24.dp), color = cardColor(), modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(20.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text("${CloudStore.credits(credits)} credits", style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text("Never expire · ${hours(credits)}", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                Spacer(Modifier.width(12.dp))
                PackStepper(packs, MAX_PACKS, onPacks)
            }
            Spacer(Modifier.height(16.dp))
            Button(onClick = onBuy, enabled = enabled && !busy, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
                if (busy) LoadingIndicator(Modifier.size(24.dp), color = ButtonDefaults.buttonColors().contentColor)
                else Text("Buy for ${CloudStore.price(b.packPriceCents * packs.toUInt())}", style = MaterialTheme.typography.titleSmall)
            }
        }
    }
}

/** − count + in one tonal pill. */
@Composable
private fun PackStepper(count: Int, max: Int, onChange: (Int) -> Unit) {
    Surface(shape = CircleShape, color = chipContainer()) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = { onChange((count - 1).coerceAtLeast(1)) }, enabled = count > 1, modifier = Modifier.alpha(if (count > 1) 1f else 0.4f)) {
                ZIcon(ZIcons.Minus, "Fewer packs", Modifier.size(18.dp))
            }
            Text("$count", style = MaterialTheme.typography.titleMedium, modifier = Modifier.widthIn(min = 22.dp))
            IconButton(onClick = { onChange((count + 1).coerceAtMost(max)) }, enabled = count < max, modifier = Modifier.alpha(if (count < max) 1f else 0.4f)) {
                ZIcon(ZIcons.Plus, "More packs", Modifier.size(18.dp))
            }
        }
    }
}

