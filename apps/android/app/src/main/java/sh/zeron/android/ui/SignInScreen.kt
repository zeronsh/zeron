package sh.zeron.android.ui

import android.net.Uri
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.material3.toShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.colorResource
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.isTraversalGroup
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.traversalIndex
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import sh.zeron.android.R
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.ServerInfo
import uniffi.zeron_core.serverSelfHostingGuide

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SignInScreen(model: AppModel) {
    val context = LocalContext.current
    val error by model.signInError.collectAsState()
    val orgs by model.orgChoice.collectAsState()
    val server by model.server.collectAsState()
    val signingIn by model.signingIn.collectAsState()
    Box(Modifier.fillMaxSize()) {
        BoxWithConstraints(Modifier.fillMaxSize().safeDrawingPadding().semantics { isTraversalGroup = true }) {
            // Scrolls only when it can't fit (a large font, a short window); otherwise centered as ever.
            Column(
                Modifier
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .heightIn(min = maxHeight)
                    // Clear of the menu above, and as much below so the center doesn't move.
                    .padding(horizontal = 28.dp, vertical = 56.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center,
            ) {
                ServerIdentity(
                    serverIdentity(server),
                    // A server fixed at launch isn't changed here; nor is one a sign-in is under way on.
                    onClick = if (model.serverFromLaunch || signingIn) null else model::openServerSheet,
                )
                Image(
                    painterResource(R.drawable.ic_launcher_foreground),
                    null,
                    Modifier.size(148.dp)
                        .clip(MaterialShapes.Cookie12Sided.toShape())
                        .background(colorResource(R.color.icon_background)),
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
                    onClick = {
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
                    onClick = { model.startDemo() },
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
        }
        WelcomeMenu(model, visible = !signingIn)
    }
    orgs?.let { (list, choice) ->
        AlertDialog(
            onDismissRequest = { choice.complete(null) },
            title = { Text("Choose an organization") },
            text = {
                Column {
                    for (org in list) {
                        ListItem(
                            headlineContent = { Text(org.name) },
                            trailingContent = {
                                TextButton(onClick = { choice.complete(org) }) { Text("Open") }
                            },
                        )
                    }
                }
            },
            confirmButton = {},
            dismissButton = { TextButton(onClick = { choice.complete(null) }) { Text("Cancel") } },
        )
    }
}

/**
 * The welcome's ⋮: the way to a server other than Zeron's, which the screen
 * otherwise never mentions. Away while a sign-in is under way (the server
 * can't change under it).
 */
@Composable
private fun BoxScope.WelcomeMenu(model: AppModel, visible: Boolean) {
    val context = LocalContext.current
    var open by remember { mutableStateOf(false) }
    LaunchedEffect(visible) { if (!visible) open = false }
    AnimatedVisibility(
        visible,
        Modifier
            .align(Alignment.TopEnd)
            .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.End))
            // Where a top app bar puts its last action.
            .padding(horizontal = 4.dp, vertical = 8.dp)
            // TalkBack reads the screen (its server first) before the menu, though it sits on top.
            .semantics { traversalIndex = 1f },
        enter = fadeIn(MaterialTheme.motionScheme.defaultEffectsSpec()),
        exit = fadeOut(MaterialTheme.motionScheme.defaultEffectsSpec()),
    ) {
        Box {
            IconButton(onClick = { open = true }, modifier = Modifier.testTag("welcome-more")) {
                ZIcon(ZIcons.MoreVertical, "More options", tint = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            ActionMenu(
                open,
                { open = false },
                listOf(
                    MenuAction("Connect to server…", ZIcons.Server, testTag = "connect-to-server", onClick = model::openServerSheet),
                    MenuAction("Self-hosting guide", ZIcons.Guide, testTag = "self-hosting-guide") {
                        model.browser.open(context, serverSelfHostingGuide())
                    },
                ),
            )
        }
    }
}

/** The welcome's identity chip: the server's label, and its host unless the label is the host. */
internal data class ServerIdentity(val name: String, val host: String?) {
    /** "Server, Acme, zeron.acme.com". */
    val description: String get() = listOfNotNull("Server", name, host).joinToString(", ")
}

/** Zeron's own server has none: the welcome never mentions a server nobody chose. */
internal fun serverIdentity(server: ServerInfo): ServerIdentity? =
    if (server.isOfficial) null else ServerIdentity(server.label, server.host.takeUnless { it == server.label })

/**
 * `[server] Acme · zeron.acme.com` above the logo, when the server in use
 * isn't Zeron's. It grows in (and out, or crossfades to another server) with
 * the screen's motion; with none it takes no room, so the hero sits where it
 * always has. [onClick] null: shown, not changed here.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun ServerIdentity(identity: ServerIdentity?, onClick: (() -> Unit)?) {
    val motion = MaterialTheme.motionScheme
    AnimatedContent(
        identity,
        transitionSpec = {
            (fadeIn(motion.defaultEffectsSpec()) togetherWith fadeOut(motion.fastEffectsSpec()))
                .using(SizeTransform(clip = false) { _, _ -> motion.defaultSpatialSpec() })
        },
        label = "server identity",
    ) { shown ->
        if (shown != null) {
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                ServerChip(shown, onClick)
                // (The chip's touch target adds 6dp of its own: 24dp to the logo, as seen.)
                Spacer(Modifier.height(18.dp))
            }
        }
    }
}

@Composable
private fun ServerChip(identity: ServerIdentity, onClick: (() -> Unit)?) {
    val muted = MaterialTheme.colorScheme.onSurfaceVariant
    val label = MaterialTheme.typography.labelLarge
    Row(
        Modifier
            .testTag("server-identity")
            .minimumInteractiveComponentSize()
            .clip(CircleShape)
            .background(chipContainer())
            .then(if (onClick != null) Modifier.clickable(onClickLabel = "Change server", role = Role.Button, onClick = onClick) else Modifier)
            .semantics(mergeDescendants = true) { contentDescription = identity.description }
            .heightIn(min = 36.dp)
            .padding(start = 12.dp, end = 16.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        ZIcon(ZIcons.Server, null, Modifier.size(18.dp), tint = muted)
        Spacer(Modifier.width(8.dp))
        Text(
            identity.name,
            style = label,
            color = MaterialTheme.colorScheme.onSurface,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.clearAndSetSemantics {},
        )
        identity.host?.let { host ->
            // The host gives way first when the two don't fit.
            Text(
                " · $host",
                style = label.copy(fontWeight = FontWeight.Normal),
                color = muted,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f, fill = false).clearAndSetSemantics {},
            )
        }
    }
}
