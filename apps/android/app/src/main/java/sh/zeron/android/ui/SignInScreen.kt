package sh.zeron.android.ui

import sh.zeron.android.feedback.tapAction
import sh.zeron.android.feedback.feedbackAction
import sh.zeron.android.feedback.Haptic
import sh.zeron.android.feedback.Cue
import sh.zeron.android.feedback.OpenCloseFeedback
import android.net.Uri
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.LinearWavyProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.toShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import sh.zeron.android.R
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.PhoneEngine
import sh.zeron.android.core.SignIn
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import sh.zeron.runtime.RuntimeState

/**
 * First run: sign in (through this phone's engine), continue without an
 * account (this phone's local workspace), or look around the demo. The
 * engine sets itself up meanwhile; its progress sits under the buttons.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SignInScreen(model: AppModel) {
    val context = LocalContext.current
    val signIn by model.signIn.collectAsState()
    val orgs by model.orgChoice.collectAsState()
    val engine by model.phone.state.collectAsState()
    val ask = rememberPermissionAsk()
    val openBrowser: (String) -> Unit = { url ->
        CustomTabsIntent.Builder().setShowTitle(true).build().launchUrl(context, Uri.parse(url))
    }
    val busy = signIn is SignIn.Busy
    Column(
        Modifier.fillMaxSize().safeDrawingPadding().padding(horizontal = 28.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Image(
            painterResource(R.mipmap.ic_launcher_foreground),
            null,
            Modifier.size(148.dp).clip(MaterialShapes.Cookie12Sided.toShape()),
        )
        Spacer(Modifier.height(32.dp))
        Text("Zeron", style = MaterialTheme.typography.displayMedium)
        Spacer(Modifier.height(10.dp))
        Text(
            "Run and steer coding agents on this phone and your computers.",
            style = MaterialTheme.typography.bodyLarge,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
        )
        Spacer(Modifier.height(48.dp))
        val tall = ButtonDefaults.MediumContainerHeight
        Button(
            onClick = tapAction { ask(false) { model.signIn(openBrowser) } },
            enabled = !busy,
            modifier = Modifier.fillMaxWidth().heightIn(min = tall),
            shapes = ButtonDefaults.shapes(),
            contentPadding = ButtonDefaults.contentPaddingFor(tall),
        ) {
            if (busy) {
                LoadingIndicator(Modifier.size(ButtonDefaults.iconSizeFor(tall)))
                Spacer(Modifier.size(ButtonDefaults.iconSpacingFor(tall)))
            }
            Text(
                when (signIn) {
                    SignIn.Preparing -> "Preparing this phone…"
                    SignIn.Completing, SignIn.Restarting -> "Signing in…"
                    else -> "Sign in"
                },
                style = ButtonDefaults.textStyleFor(tall),
            )
        }
        Spacer(Modifier.height(12.dp))
        // No account: this phone's own workspace, like the desktop's local profile.
        FilledTonalButton(
            onClick = feedbackAction(Haptic.Confirm, Cue.Open) { ask(true) { model.continueLocally() } },
            enabled = !busy && model.phone.isSupportedAbi,
            modifier = Modifier.fillMaxWidth().heightIn(min = tall),
            shapes = ButtonDefaults.shapes(),
            contentPadding = ButtonDefaults.contentPaddingFor(tall),
        ) {
            Text("Continue without an account", style = ButtonDefaults.textStyleFor(tall))
        }
        Spacer(Modifier.height(12.dp))
        OutlinedButton(
            onClick = feedbackAction(Haptic.Confirm, Cue.Open) { model.startDemo() },
            enabled = !busy,
            modifier = Modifier.fillMaxWidth().heightIn(min = tall),
            shapes = ButtonDefaults.shapes(),
        ) {
            Text("Explore the demo", style = ButtonDefaults.textStyleFor(tall))
        }
        (signIn as? SignIn.Failed)?.let {
            Spacer(Modifier.height(20.dp))
            Text(it.message, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium, textAlign = TextAlign.Center)
        }
        Spacer(Modifier.height(32.dp))
        EngineStatusStrip(model, engine)
    }
    orgs?.let { (list, choice) -> OrgDialog(list, onPick = { choice.complete(it) }) }
}

/**
 * This phone's engine in one line: setting up (with progress), starting,
 * ready, or stopped with a way back. Compact: it never blocks the screen.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun EngineStatusStrip(model: AppModel, state: RuntimeState, modifier: Modifier = Modifier) {
    if (!model.phone.isSupportedAbi) return
    val busy = state is RuntimeState.Bootstrapping || state == RuntimeState.Starting
    val failed = state is RuntimeState.Failed
    Surface(
        shape = RoundedCornerShape(20.dp),
        color = if (failed) MaterialTheme.colorScheme.errorContainer else MaterialTheme.colorScheme.surfaceContainerHigh,
        modifier = modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(horizontal = 16.dp, vertical = 12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (busy) {
                    LoadingIndicator(Modifier.size(24.dp))
                } else {
                    ZIcon(
                        if (failed) ZIcons.Warning else ZIcons.Phone,
                        null,
                        Modifier.size(20.dp),
                        tint = if (failed) MaterialTheme.colorScheme.onErrorContainer else MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Spacer(Modifier.width(12.dp))
                Text(
                    engineStatusLine(state),
                    style = MaterialTheme.typography.bodyMedium,
                    color = if (failed) MaterialTheme.colorScheme.onErrorContainer else MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.weight(1f),
                    maxLines = 2,
                )
                if (failed || state == RuntimeState.Stopped) {
                    TextButton(onClick = feedbackAction(Haptic.Confirm, if (failed) Cue.Refresh else Cue.ToggleOn) { model.startEngine() }) { Text(if (failed) "Try again" else "Start") }
                }
            }
            AnimatedVisibility(state is RuntimeState.Bootstrapping) {
                val progress = (state as? RuntimeState.Bootstrapping)?.progress
                Column {
                    Spacer(Modifier.height(10.dp))
                    if (progress != null) {
                        LinearWavyProgressIndicator(progress = { progress }, modifier = Modifier.fillMaxWidth())
                    } else {
                        LinearWavyProgressIndicator(Modifier.fillMaxWidth())
                    }
                }
            }
        }
    }
}

fun engineStatusLine(state: RuntimeState): String = when (state) {
    RuntimeState.NotInstalled -> "This phone will run agents too"
    is RuntimeState.Bootstrapping ->
        "Setting up this phone · ${state.step}" + (state.progress?.let { " · ${(it * 100).toInt()}%" } ?: "")
    RuntimeState.Starting -> "Starting this phone's engine…"
    is RuntimeState.Running -> "This phone is ready to run agents"
    RuntimeState.Stopped -> "This phone's engine is stopped"
    is RuntimeState.Failed -> "${PhoneEngine.stateLabel(state)}: ${state.reason}"
}

@Composable
fun OrgDialog(list: List<uniffi.zeron_core.AuthOrg>, onPick: (uniffi.zeron_core.AuthOrg?) -> Unit) {
    AlertDialog(
        onDismissRequest = { onPick(null) },
        title = { Text("Choose an organization") },
        text = {
            OpenCloseFeedback()
            Column {
                for (org in list) {
                    ListItem(
                        headlineContent = { Text(org.name) },
                        trailingContent = {
                            TextButton(onClick = tapAction { onPick(org) }) { Text("Open") }
                        },
                    )
                }
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onClick = tapAction { onPick(null) }) { Text("Cancel") } },
    )
}
