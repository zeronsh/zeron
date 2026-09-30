package sh.zeron.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ButtonGroupDefaults
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.ToggleButton
import androidx.compose.material3.ToggleButtonDefaults
import androidx.compose.material3.toShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.ThemeMode
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.coreVersion
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SettingsScreen(model: AppModel, onOpen: (String) -> Unit) {
    val appearance by model.appearance.collectAsState()
    val workspace by model.workspace.collectAsState()
    val devices = workspace?.devices.orEmpty()
    val list = androidx.compose.foundation.lazy.rememberLazyListState()
    Box(Modifier.fillMaxSize()) {
    LazyColumn(
        Modifier.fillMaxSize(),
        state = list,
        contentPadding = PaddingValues(top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 8.dp, bottom = 140.dp),
    ) {
        item { ScreenHeader("Settings", null) }
        item {
            // Account: a tonal hero card with a shaped monogram.
            Surface(
                shape = RoundedCornerShape(32.dp),
                color = MaterialTheme.colorScheme.primaryContainer,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
            ) {
                Row(Modifier.padding(20.dp), verticalAlignment = Alignment.CenterVertically) {
                    Box(
                        Modifier.size(64.dp).clip(MaterialShapes.Cookie9Sided.toShape()).background(MaterialTheme.colorScheme.primary),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(model.accountName.take(1).uppercase(), style = MaterialTheme.typography.headlineSmallEmphasized, color = MaterialTheme.colorScheme.onPrimary)
                    }
                    Spacer(Modifier.width(16.dp))
                    Column {
                        Text(model.accountName, style = MaterialTheme.typography.titleLargeEmphasized, color = MaterialTheme.colorScheme.onPrimaryContainer)
                        Text(model.accountDetail, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onPrimaryContainer.copy(alpha = 0.8f))
                    }
                }
            }
        }
        section("Agents")
        item {
            // The agents on any engine device (Demo: a simulated one).
            Column(Modifier.padding(horizontal = 16.dp)) {
                SegmentedListItem(
                    onClick = { onOpen(Routes.AGENTS) },
                    shapes = segmentedShapes(0, 1),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Bot) },
                    supportingContent = { Text("Install agents and sign in to their accounts") },
                    trailingContent = { ZIcon(ZIcons.ChevronRight, null, Modifier.size(20.dp)) },
                ) { Text("Coding agents") }
            }
        }
        section("Appearance")
        item {
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                Surface(shape = segmentedShapes(0, 2).shape, color = cardColor()) {
                    Column(Modifier.padding(16.dp)) {
                        Text("Theme", style = MaterialTheme.typography.titleMedium)
                        Spacer(Modifier.height(12.dp))
                        Row(horizontalArrangement = Arrangement.spacedBy(ButtonGroupDefaults.ConnectedSpaceBetween)) {
                            val modes = listOf(
                                Triple(ThemeMode.System, "System", ZIcons.Monitor),
                                Triple(ThemeMode.Light, "Light", ZIcons.Sun),
                                Triple(ThemeMode.Dark, "Dark", ZIcons.Moon),
                            )
                            modes.forEachIndexed { index, (mode, label, icon) ->
                                ToggleButton(
                                    checked = appearance.mode == mode,
                                    onCheckedChange = { model.setAppearance(appearance.copy(mode = mode)) },
                                    modifier = Modifier.weight(1f).semantics { role = Role.RadioButton },
                                    shapes = when (index) {
                                        0 -> ButtonGroupDefaults.connectedLeadingButtonShapes()
                                        modes.lastIndex -> ButtonGroupDefaults.connectedTrailingButtonShapes()
                                        else -> ButtonGroupDefaults.connectedMiddleButtonShapes()
                                    },
                                ) {
                                    ZIcon(icon, null, Modifier.size(ToggleButtonDefaults.IconSize))
                                    Spacer(Modifier.size(ToggleButtonDefaults.IconSpacing))
                                    Text(label)
                                }
                            }
                        }
                    }
                }
                SegmentedListItem(
                    onClick = { model.setAppearance(appearance.copy(dynamicColor = !appearance.dynamicColor)) },
                    shapes = segmentedShapes(1, 2),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Magic) },
                    supportingContent = { Text("Tint Zeron with your wallpaper's palette") },
                    trailingContent = { Switch(appearance.dynamicColor, { model.setAppearance(appearance.copy(dynamicColor = it)) }) },
                ) { Text("Wallpaper colors") }
            }
        }
        section("Wallpaper")
        item { WallpaperSettings(model) }
        if (devices.isNotEmpty()) {
            section("Devices")
            item {
                Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                    devices.forEachIndexed { i, device ->
                        SegmentedListItem(
                            onClick = {},
                            shapes = segmentedShapes(i, devices.size),
                            colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                            leadingContent = {
                                IconTile(
                                    when {
                                        !device.isExecutionHost -> ZIcons.Phone
                                        device.platform == "linux" -> ZIcons.Server
                                        else -> ZIcons.Laptop
                                    },
                                )
                            },
                            supportingContent = {
                                Text(
                                    listOfNotNull(
                                        if (device.isSelf) "This device" else if (device.online) "Online" else "Offline",
                                        device.version?.let { "v$it" },
                                        if (device.sessionCount > 0u) "${device.sessionCount} sessions" else null,
                                    ).joinToString(" · "),
                                )
                            },
                            trailingContent = {
                                Box(
                                    Modifier.size(10.dp).clip(CircleShape).background(
                                        if (device.online || device.isSelf) successColor() else MaterialTheme.colorScheme.outlineVariant,
                                    ),
                                )
                            },
                        ) { Text(device.name) }
                    }
                }
            }
        }
        section("About")
        item {
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                SegmentedListItem(
                    onClick = {},
                    shapes = segmentedShapes(0, 2),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Info) },
                    supportingContent = { Text("Zeron for Android · core ${coreVersion()}") },
                ) { Text("Version") }
                SegmentedListItem(
                    onClick = { model.signOut() },
                    shapes = segmentedShapes(1, 2),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Logout, MaterialTheme.colorScheme.errorContainer, MaterialTheme.colorScheme.onErrorContainer) },
                ) { Text(if (model.isDemo) "Leave demo" else "Sign out", color = MaterialTheme.colorScheme.error) }
            }
        }
    }
    StatusBarScrim(scrolled = list.firstVisibleItemIndex > 0 || list.firstVisibleItemScrollOffset > 0)
    }
}

private fun LazyListScope.section(title: String) {
    item {
        Text(
            title,
            style = MaterialTheme.typography.titleSmallEmphasized,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.padding(start = 28.dp, top = 24.dp, bottom = 8.dp),
        )
    }
}

/** A leading glyph on a tonal rounded tile. */
@Composable
fun IconTile(
    icon: Int,
    container: Color = MaterialTheme.colorScheme.secondaryContainer,
    content: Color = MaterialTheme.colorScheme.onSecondaryContainer,
) {
    Box(Modifier.size(40.dp).clip(RoundedCornerShape(14.dp)).background(container), contentAlignment = Alignment.Center) {
        ZIcon(icon, null, Modifier.size(22.dp), tint = content)
    }
}

/** Wallpaper: shown behind new sessions and the sessions list (iOS parity). */
@Composable
private fun WallpaperSettings(model: AppModel) {
    val store = model.wallpaper
    val state by store.state.collectAsState()
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    val picker = androidx.activity.compose.rememberLauncherForActivityResult(
        androidx.activity.result.contract.ActivityResultContracts.PickVisualMedia(),
    ) { uri -> if (uri != null) scope.launch { store.set(uri, "Photo") } }
    var effects by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf(false) }
    val rows = if (state.set) 3 else 1
    Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
        SegmentedListItem(
            onClick = {
                picker.launch(androidx.activity.result.PickVisualMediaRequest(androidx.activity.result.contract.ActivityResultContracts.PickVisualMedia.ImageOnly))
            },
            shapes = segmentedShapes(0, rows),
            colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
            leadingContent = { IconTile(ZIcons.Image) },
            supportingContent = { Text(if (state.set) state.name ?: "Photo" else "Shown behind new sessions and the sessions list") },
        ) { Text(if (state.set) "Change wallpaper" else "Choose wallpaper") }
        if (state.set) {
            Box {
                SegmentedListItem(
                    onClick = { effects = true },
                    shapes = segmentedShapes(1, rows),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Magic) },
                    supportingContent = { Text("${sh.zeron.android.core.WallpaperStore.label(state.effect)} — ${sh.zeron.android.core.WallpaperStore.detail(state.effect)}") },
                ) { Text("Effect") }
                ChoiceMenu(effects, { effects = false }, listOf(MenuSection("Effect", sh.zeron.android.core.WallpaperStore.effects.map { e ->
                    MenuChoice(sh.zeron.android.core.WallpaperStore.label(e), e == state.effect) { store.setEffect(e) }
                })))
            }
            SegmentedListItem(
                onClick = { store.remove() },
                shapes = segmentedShapes(2, rows),
                colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                leadingContent = { IconTile(ZIcons.Delete, MaterialTheme.colorScheme.errorContainer, MaterialTheme.colorScheme.onErrorContainer) },
            ) { Text("Remove wallpaper", color = MaterialTheme.colorScheme.error) }
        }
    }
}
