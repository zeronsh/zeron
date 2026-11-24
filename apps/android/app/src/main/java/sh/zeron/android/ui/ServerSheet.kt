package sh.zeron.android.ui

import android.view.HapticFeedbackConstants
import androidx.annotation.DrawableRes
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.ModalBottomSheetProperties
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.SheetValue
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.isSameAs
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.ServerInfo
import uniffi.zeron_core.ServerLookupException
import uniffi.zeron_core.serverDiscover
import uniffi.zeron_core.serverSelfHostingGuide

/** What choosing leads to: where the next sign-in goes, or (signed in) signing out to switch. */
enum class ServerSheetMode { SignIn, Switch }

/**
 * The confirm button's words. [found]: a link's server, looked up and waiting
 * for the tap that chooses it.
 */
internal fun serverConfirmLabel(mode: ServerSheetMode, selfHosted: Boolean, found: ServerInfo?): String = when {
    found != null -> "Connect to ${found.label}"
    mode == ServerSheetMode.Switch -> "Continue"
    selfHosted -> "Connect"
    else -> "Use Zeron"
}

/**
 * Choose a server: Zeron's own, or a self-hosted one by its address. The core
 * reads the address's discovery document and words every failure, shown as is.
 * A [request] from a `zeron://server` link fills the address in and looks it
 * up at once; what it finds still waits for a tap.
 */
@Composable
fun ServerSheet(model: AppModel, mode: ServerSheetMode, request: AppModel.ServerSheetRequest, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val view = LocalView.current
    val focusManager = LocalFocusManager.current
    // Leaving composition cancels a lookup in flight.
    val scope = rememberCoroutineScope()
    val current by model.server.collectAsState()
    val locked = model.serverFromLaunch
    // (A fixed server leaves a link's address aside, as it does everything else.)
    val linked = request.address?.takeUnless { locked }
    var selfHosted by remember { mutableStateOf(linked != null || !current.isOfficial) }
    var address by remember { mutableStateOf(linked ?: if (current.isOfficial) "" else current.url) }
    var error by remember { mutableStateOf<String?>(null) }
    var checking by remember { mutableStateOf(linked != null) }
    var lookup by remember { mutableStateOf<Job?>(null) }
    var found by remember { mutableStateOf<ServerInfo?>(null) }
    var switchTo by remember { mutableStateOf<ServerInfo?>(null) }
    var focusOnReveal by remember { mutableStateOf(false) }
    val field = remember { FocusRequester() }
    // While checking, no swipe takes the sheet away (nor, below, a tap outside or back).
    val sheet = rememberBottomSheetState(
        initialValue = SheetValue.Hidden,
        enabledValues = setOf(SheetValue.Hidden, SheetValue.Expanded),
        confirmValueChange = { it != SheetValue.Hidden || !checking },
    )

    fun close() {
        scope.launch { sheet.hide() }.invokeOnCompletion { onDismiss() }
    }

    fun use(server: ServerInfo) {
        when {
            server.isSameAs(current) -> close()
            mode == ServerSheetMode.Switch -> switchTo = server
            else -> {
                model.chooseServer(server)
                close()
            }
        }
    }

    /** A link's server, found: never chosen without a tap. */
    fun offer(server: ServerInfo) {
        when {
            server.isSameAs(current) -> close()
            // Signed in, the switch dialog is the tap.
            mode == ServerSheetMode.Switch -> switchTo = server
            else -> found = server
        }
    }

    fun lookUp(typed: String, then: (ServerInfo) -> Unit) {
        // A newer link replaces a lookup in flight.
        lookup?.cancel()
        error = null
        checking = true
        lookup = scope.launch {
            val server = try {
                serverDiscover(typed)
            } catch (e: ServerLookupException.Failed) {
                view.performHapticFeedback(HapticFeedbackConstants.REJECT)
                error = e.reason
                null
            } catch (e: CancellationException) {
                // (Not reset: the lookup that replaced this one is checking now.)
                throw e
            } catch (e: Exception) {
                // Not the address's fault (the core failed): said, not thrown.
                view.performHapticFeedback(HapticFeedbackConstants.REJECT)
                error = "Couldn't check that server. Try again."
                null
            }
            checking = false
            server?.let(then)
        }
    }

    fun submit() {
        if (checking || locked) return
        found?.let { return use(it) }
        if (!selfHosted) return use(model.officialServer)
        val typed = address.trim()
        if (typed.isEmpty()) return
        // (The current address is looked up too: its sign-in client may have
        // changed under it.)
        lookUp(typed, ::use)
    }

    // A link (the one that opened the sheet, or another while it's open) fills
    // the address in and looks it up, whatever was there.
    LaunchedEffect(request.serial) {
        val linkAddress = linked ?: return@LaunchedEffect
        selfHosted = true
        address = linkAddress
        found = null
        switchTo = null
        focusManager.clearFocus()
        lookUp(linkAddress.trim(), ::offer)
    }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheet,
        containerColor = MaterialTheme.colorScheme.surfaceContainerLow,
        properties = ModalBottomSheetProperties(shouldDismissOnBackPress = !checking, shouldDismissOnClickOutside = !checking),
    ) {
        Column(
            Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).navigationBarsPadding().padding(start = 16.dp, end = 16.dp, bottom = 16.dp),
        ) {
            Text("Choose a server", style = MaterialTheme.typography.headlineSmallEmphasized, modifier = Modifier.padding(horizontal = 8.dp))
            Spacer(Modifier.height(8.dp))
            Text(
                "Zeron keeps your chats in sync through a server. Use Zeron's, or one you or your team runs.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 8.dp),
            )
            if (locked) {
                Spacer(Modifier.height(12.dp))
                Row(Modifier.padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                    ZIcon(ZIcons.Info, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                    Spacer(Modifier.width(8.dp))
                    Text("Set by a launch argument", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
            Spacer(Modifier.height(20.dp))
            Column(Modifier.selectableGroup(), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                ServerOption(!selfHosted, 0, ZIcons.Zeron, "Zeron", model.officialServer.host, enabled = !locked && !checking) {
                    selfHosted = false
                    error = null
                    found = null
                    focusManager.clearFocus()
                }
                ServerOption(selfHosted, 1, ZIcons.Server, "Self-hosted", "Your own server", enabled = !locked && !checking) {
                    if (!selfHosted) {
                        focusOnReveal = true
                        found = null
                    }
                    selfHosted = true
                }
            }
            AnimatedVisibility(selfHosted, enter = fadeIn() + expandVertically(), exit = fadeOut() + shrinkVertically()) {
                Column(Modifier.padding(top = 16.dp)) {
                    OutlinedTextField(
                        address,
                        {
                            // Another address is another server: back to a plain Connect.
                            if (it != address) found = null
                            address = it
                            error = null
                        },
                        enabled = !checking && !locked,
                        singleLine = true,
                        label = { Text("Server address") },
                        placeholder = { Text("zeron.example.com") },
                        isError = error != null,
                        supportingText = error?.let { { Text(it) } },
                        keyboardOptions = KeyboardOptions(
                            capitalization = KeyboardCapitalization.None,
                            autoCorrectEnabled = false,
                            keyboardType = KeyboardType.Uri,
                            imeAction = ImeAction.Go,
                        ),
                        keyboardActions = KeyboardActions(onGo = { submit() }),
                        shape = RoundedCornerShape(16.dp),
                        modifier = Modifier.fillMaxWidth().focusRequester(field).testTag("server-address"),
                    )
                    Spacer(Modifier.height(8.dp))
                    Text(
                        "Your account on a self-hosted server is separate from your Zeron account.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(horizontal = 16.dp),
                    )
                    // Revealed by a tap, or back from a failed lookup (checking took the focus away).
                    LaunchedEffect(focusOnReveal, error) {
                        if (focusOnReveal || error != null) field.requestFocus()
                        focusOnReveal = false
                    }
                }
            }
            Spacer(Modifier.height(24.dp))
            PrimaryCapsule(
                serverConfirmLabel(mode, selfHosted, found),
                ::submit,
                enabled = !locked && (!selfHosted || address.isNotBlank()),
                busy = checking,
                busyText = "Connecting…",
            )
            Spacer(Modifier.height(8.dp))
            PlainCapsule("Run your own server", { model.browser.open(context, serverSelfHostingGuide()) })
        }
    }

    switchTo?.let { target ->
        AlertDialog(
            onDismissRequest = { switchTo = null },
            icon = { ZIcon(ZIcons.Logout, null) },
            title = { Text("Switch to ${target.label}?") },
            text = { Text("You'll be signed out here, then sign in on ${target.label}. Local drafts stay on this device.") },
            confirmButton = {
                TextButton(onClick = {
                    switchTo = null
                    scope.launch { sheet.hide() }.invokeOnCompletion {
                        onDismiss()
                        model.switchServer(target)
                    }
                }) { Text("Sign Out and Switch", color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { TextButton(onClick = { switchTo = null }) { Text("Cancel") } },
        )
    }
}

/** One of the two choices, marked as NewSession's project rows mark theirs. */
@Composable
private fun ServerOption(
    selected: Boolean,
    index: Int,
    @DrawableRes icon: Int,
    title: String,
    detail: String,
    enabled: Boolean,
    onClick: () -> Unit,
) {
    val container = if (selected) MaterialTheme.colorScheme.secondaryContainer else cardColor()
    SegmentedListItem(
        onClick = onClick,
        enabled = enabled,
        shapes = segmentedShapes(index, 2),
        colors = ListItemDefaults.segmentedColors(containerColor = container, disabledContainerColor = container),
        // The tile swaps with the row's tone so it still stands out on the selected row.
        leadingContent = { if (selected) IconTile(icon, container = cardColor()) else IconTile(icon) },
        supportingContent = { Text(detail, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        trailingContent = if (selected) {
            {
                Box(
                    Modifier.size(28.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary),
                    contentAlignment = Alignment.Center,
                ) { ZIcon(ZIcons.Check, null, Modifier.size(16.dp), tint = MaterialTheme.colorScheme.onPrimary) }
            }
        } else {
            null
        },
        modifier = Modifier.semantics {
            role = Role.RadioButton
            this.selected = selected
        },
    ) { Text(title, style = MaterialTheme.typography.titleMedium) }
}
