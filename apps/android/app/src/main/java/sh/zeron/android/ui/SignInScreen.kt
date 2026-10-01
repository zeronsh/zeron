package sh.zeron.android.ui

import sh.zeron.android.feedback.tapAction
import sh.zeron.android.feedback.feedbackAction
import sh.zeron.android.feedback.LocalFeedback
import sh.zeron.android.feedback.Haptic
import sh.zeron.android.feedback.Cue
import sh.zeron.android.feedback.OpenCloseFeedback
import android.net.Uri
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.toShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import sh.zeron.android.R
import sh.zeron.android.core.AppModel

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SignInScreen(model: AppModel) {
    val context = LocalContext.current
    val error by model.signInError.collectAsState()
    val orgs by model.orgChoice.collectAsState()
    // Developer sign-in (debuggable builds): seven taps on the mark.
    var taps by remember { mutableIntStateOf(0) }
    var developer by remember { mutableStateOf(false) }
    val fb = LocalFeedback.current
    Column(
        Modifier.fillMaxSize().safeDrawingPadding().padding(horizontal = 28.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Image(
            painterResource(R.mipmap.ic_launcher_foreground),
            null,
            Modifier.size(148.dp).clip(MaterialShapes.Cookie12Sided.toShape()).clickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
            ) {
                if (model.isDebuggable) {
                    // The hidden developer door: a tick per tap, a confirmation when it opens.
                    if (++taps >= 7) {
                        developer = true
                        fb.both(Haptic.Success, Cue.Open)
                    } else {
                        fb.haptic(Haptic.Tick)
                    }
                }
            },
        )
        Spacer(Modifier.height(32.dp))
        Text("Zeron", style = MaterialTheme.typography.displayMedium)
        Spacer(Modifier.height(10.dp))
        Text(
            "Follow and steer your coding agents from anywhere.",
            style = MaterialTheme.typography.bodyLarge,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
        )
        Spacer(Modifier.height(48.dp))
        Button(
            onClick = tapAction {
                val url = model.beginSignIn()
                CustomTabsIntent.Builder().setShowTitle(true).build().launchUrl(context, Uri.parse(url))
            },
            modifier = Modifier.fillMaxWidth().heightIn(min = ButtonDefaults.MediumContainerHeight),
            shapes = ButtonDefaults.shapes(),
            contentPadding = ButtonDefaults.contentPaddingFor(ButtonDefaults.MediumContainerHeight),
        ) {
            Text("Sign in", style = ButtonDefaults.textStyleFor(ButtonDefaults.MediumContainerHeight))
        }
        Spacer(Modifier.height(12.dp))
        OutlinedButton(
            onClick = feedbackAction(Haptic.Confirm, Cue.Open) { model.startDemo() },
            modifier = Modifier.fillMaxWidth().heightIn(min = ButtonDefaults.MediumContainerHeight),
            shapes = ButtonDefaults.shapes(),
        ) {
            Text("Explore the demo", style = ButtonDefaults.textStyleFor(ButtonDefaults.MediumContainerHeight))
        }
        error?.let {
            Spacer(Modifier.height(20.dp))
            Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium, textAlign = TextAlign.Center)
        }
    }
    if (developer) DevSignInDialog(onDismiss = { developer = false }) { edge, user, org ->
        developer = false
        model.devSignIn(edge, user, org)
    }
    orgs?.let { (list, choice) ->
        AlertDialog(
            onDismissRequest = { choice.complete(null) },
            title = { Text("Choose an organization") },
            text = {
                OpenCloseFeedback()
                Column {
                    for (org in list) {
                        ListItem(
                            headlineContent = { Text(org.name) },
                            trailingContent = {
                                TextButton(onClick = tapAction { choice.complete(org) }) { Text("Open") }
                            },
                        )
                    }
                }
            },
            confirmButton = {},
            dismissButton = { TextButton(onClick = tapAction { choice.complete(null) }) { Text("Cancel") } },
        )
    }
}

/**
 * Developer: an `AUTH_MODE=dev` edge (e.g. `wrangler dev --var AUTH_MODE:dev`
 * in edge/) and the `user@org` to be — the bearer such an edge accepts.
 */
@Composable
private fun DevSignInDialog(onDismiss: () -> Unit, onSignIn: (String, String, String) -> Unit) {
    var edge by remember { mutableStateOf("http://10.0.2.2:27740") }
    var user by remember { mutableStateOf("dev-user") }
    var org by remember { mutableStateOf("dev-org") }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Developer sign-in") },
        text = {
            OpenCloseFeedback()
            Column {
                Text(
                    "Join a development edge (AUTH_MODE=dev) without WorkOS.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(12.dp))
                OutlinedTextField(edge, { edge = it }, label = { Text("Edge URL") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                OutlinedTextField(user, { user = it }, label = { Text("User id") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                OutlinedTextField(org, { org = it }, label = { Text("Organization id") }, singleLine = true, modifier = Modifier.fillMaxWidth())
            }
        },
        confirmButton = { TextButton(onClick = feedbackAction(Haptic.Confirm, Cue.Select) { onSignIn(edge, user, org) }) { Text("Sign in") } },
        dismissButton = { TextButton(onClick = tapAction(action = onDismiss)) { Text("Cancel") } },
    )
}
