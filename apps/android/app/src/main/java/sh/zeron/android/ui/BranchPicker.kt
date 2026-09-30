package sh.zeron.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.ButtonGroupDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.ToggleButton
import androidx.compose.material3.ToggleButtonDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.NewSessionDraft
import sh.zeron.android.core.userMessage
import sh.zeron.android.design.GeistMono
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.RepoRef

/** What picking a ref in a new session's branch picker does. */
sealed interface RefPick {
    /** Just record it on the draft. */
    data class Set(val draft: NewSessionDraft) : RefPick

    /** `git checkout` the project folder first, then record it. */
    data class Checkout(val name: String) : RefPick
}

/** The branch picker's rules — the desktop's `pick_ref` / `pick_checkout` (crates/ui/src/pickers.rs). */
object BranchRules {
    /** Case-insensitive match on the ref's name, keeping the engine's order (default branch first). */
    fun filter(refs: List<RepoRef>, query: String): List<RepoRef> {
        val q = query.trim()
        return if (q.isEmpty()) refs else refs.filter { it.name.contains(q, ignoreCase = true) }
    }

    /** The row wearing the check: the draft's pick, else the checked-out branch. */
    fun selected(refs: List<RepoRef>, draft: NewSessionDraft): String? = draft.branch ?: refs.firstOrNull { it.current }?.name

    /**
     * A branch with its own worktree reuses it; a new worktree's base or the
     * checked-out branch is just recorded; any other branch checks the
     * project folder out onto it (picking `main` means "put my checkout on
     * main" — it never flips the mode).
     */
    fun pick(draft: NewSessionDraft, ref: RepoRef): RefPick = when {
        ref.worktreePath != null -> RefPick.Set(draft.copy(branch = ref.name, cwd = ref.worktreePath, worktree = false))
        draft.worktree || ref.current -> RefPick.Set(draft.copy(branch = ref.name, cwd = null))
        else -> RefPick.Checkout(ref.name)
    }

    /**
     * Back to the checkout with a base the folder isn't on (and that has no
     * worktree of its own): drop it, the current branch takes over.
     */
    fun checkout(draft: NewSessionDraft, worktree: Boolean, refs: List<RepoRef>): NewSessionDraft {
        if (worktree) return draft.copy(worktree = true, cwd = null)
        val picked = refs.firstOrNull { it.name == draft.branch }
        val keep = picked != null && (picked.current || picked.worktreePath != null)
        return draft.copy(
            worktree = false,
            branch = if (keep) draft.branch else null,
            cwd = if (keep) picked?.worktreePath else null,
        )
    }

    /** The chip: the worktree's base, the picked branch, or the checked-out one. */
    fun label(draft: NewSessionDraft, refs: List<RepoRef>?): String {
        val branch = draft.branch ?: refs?.firstOrNull { it.current }?.name
        return when {
            draft.worktree -> "New worktree" + (branch?.let { " · $it" } ?: "")
            branch != null -> branch
            else -> "Branch"
        }
    }
}

/**
 * New session: the checkout (the project folder, or a new worktree) and the
 * branch, from the device's refs (local branches, then remote-only ones).
 */
@Composable
fun BranchChip(
    model: AppModel,
    draft: NewSessionDraft,
    deviceId: String,
    repoPath: String,
    onCurrent: (String?) -> Unit,
    onPick: (NewSessionDraft) -> Unit,
) {
    val client by model.client.collectAsState()
    val scope = rememberCoroutineScope()
    var open by remember { mutableStateOf(false) }
    var refs by remember(deviceId, repoPath) { mutableStateOf<List<RepoRef>?>(null) }
    var error by remember(deviceId, repoPath) { mutableStateOf<String?>(null) }
    var switching by remember { mutableStateOf<String?>(null) }
    var reload by remember { mutableIntStateOf(0) }
    // The popup's content is its own composition: read the draft as it is now.
    val latest by rememberUpdatedState(draft)
    val emit by rememberUpdatedState(onPick)
    // Loaded up front (the chip names the checked-out branch) and on each opening.
    LaunchedEffect(client, deviceId, repoPath, reload, open) {
        val c = client ?: return@LaunchedEffect
        if (!open && refs != null) return@LaunchedEffect
        runCatching { c.listRefs(deviceId, repoPath) }
            .onSuccess {
                refs = it
                error = null
            }
            .onFailure { if (refs == null || open) error = it.userMessage() }
    }

    // The checked-out branch, which a session started without a pick runs on.
    LaunchedEffect(refs) { onCurrent(refs?.firstOrNull { it.current }?.name) }

    fun pick(ref: RepoRef) {
        if (switching != null) return
        when (val action = BranchRules.pick(latest, ref)) {
            is RefPick.Set -> {
                emit(action.draft)
                open = false
            }
            is RefPick.Checkout -> {
                val c = client ?: return
                switching = action.name
                error = null
                scope.launch {
                    runCatching { c.switchRef(deviceId, repoPath, action.name) }
                        .onSuccess {
                            emit(latest.copy(branch = action.name, cwd = null, worktree = false))
                            open = false
                            reload++
                        }
                        .onFailure { error = it.userMessage() }
                    switching = null
                }
            }
        }
    }

    ContextChip(
        BranchRules.label(draft, refs),
        leading = { ZIcon(if (draft.worktree || draft.cwd != null) ZIcons.Project else ZIcons.Branch, null, Modifier.size(16.dp)) },
        onClick = { open = true },
    ) {
        AnchoredPopover(open, { if (switching == null) open = false }, maxHeight = 480.dp) {
            BranchPickerContent(
                refs = refs,
                draft = latest,
                error = error,
                switching = switching,
                onCheckout = { worktree -> emit(BranchRules.checkout(latest, worktree, refs.orEmpty())) },
                onPick = ::pick,
                onRetry = {
                    error = null
                    reload++
                },
            )
        }
    }
}

@Composable
private fun ColumnScope.BranchPickerContent(
    refs: List<RepoRef>?,
    draft: NewSessionDraft,
    error: String?,
    switching: String?,
    onCheckout: (Boolean) -> Unit,
    onPick: (RepoRef) -> Unit,
    onRetry: () -> Unit,
) {
    var query by remember { mutableStateOf("") }
    Text(
        "Checkout",
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(start = 20.dp, top = 16.dp, bottom = 8.dp),
    )
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(ButtonGroupDefaults.ConnectedSpaceBetween),
    ) {
        listOf(false to "Current checkout", true to "New worktree").forEachIndexed { i, (worktree, label) ->
            ToggleButton(
                checked = draft.worktree == worktree,
                onCheckedChange = { onCheckout(worktree) },
                shapes = if (i == 0) ButtonGroupDefaults.connectedLeadingButtonShapes() else ButtonGroupDefaults.connectedTrailingButtonShapes(),
                modifier = Modifier.weight(1f).semantics { role = Role.RadioButton },
            ) {
                ZIcon(if (worktree) ZIcons.Project else ZIcons.Folder, null, Modifier.size(ToggleButtonDefaults.IconSize))
                Spacer(Modifier.size(ToggleButtonDefaults.IconSpacing))
                Text(label, maxLines = 1)
            }
        }
    }
    Text(
        if (draft.worktree) "A fresh worktree is created from the branch below when you send." else "Runs in the project folder; picking a branch checks it out there.",
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(start = 20.dp, end = 20.dp, top = 8.dp),
    )
    // Search: an icon and a borderless field over a hairline, like the desktop's.
    Row(
        Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, top = 12.dp).clip(RoundedCornerShape(20.dp))
            .background(chipContainer()).heightIn(min = 44.dp).padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        ZIcon(ZIcons.Search, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Spacer(Modifier.width(10.dp))
        Box(Modifier.weight(1f)) {
            if (query.isEmpty()) Text("Search branches", style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
            BasicTextField(
                query,
                { query = it },
                singleLine = true,
                textStyle = MaterialTheme.typography.bodyLarge.copy(color = MaterialTheme.colorScheme.onSurface),
                cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
    error?.let {
        Row(Modifier.fillMaxWidth().padding(start = 20.dp, end = 8.dp, top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error, modifier = Modifier.weight(1f))
            if (refs == null) TextButton(onClick = onRetry) { Text("Retry") }
        }
    }
    Spacer(Modifier.size(8.dp))
    HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.6f))
    val rows = BranchRules.filter(refs.orEmpty(), query)
    val selected = BranchRules.selected(refs.orEmpty(), draft)
    LazyColumn(
        contentPadding = PaddingValues(horizontal = 8.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
        modifier = Modifier.weight(1f, fill = false),
    ) {
        when {
            refs == null && error == null -> item("loading") {
                Box(Modifier.fillMaxWidth().padding(vertical = 24.dp), contentAlignment = Alignment.Center) { LoadingIndicator(Modifier.size(32.dp)) }
            }
            refs != null && rows.isEmpty() -> item("none") {
                Text(
                    if (query.isBlank()) "No branches yet." else "No branch matches “${query.trim()}”.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.fillMaxWidth().padding(24.dp),
                )
            }
        }
        items(rows, key = { it.name }) { ref ->
            RefRow(ref, selected = ref.name == selected, busy = switching == ref.name, enabled = switching == null) { onPick(ref) }
        }
    }
}

@Composable
private fun RefRow(ref: RepoRef, selected: Boolean, busy: Boolean, enabled: Boolean, onClick: () -> Unit) {
    val content = if (selected) MaterialTheme.colorScheme.onSecondaryContainer else MaterialTheme.colorScheme.onSurface
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(20.dp))
            .background(if (selected) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent)
            .semantics { this.selected = selected }
            .clickable(enabled = enabled, role = Role.RadioButton, onClick = onClick)
            .padding(start = 14.dp, end = 14.dp, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        ZIcon(if (ref.worktreePath != null) ZIcons.Project else ZIcons.Branch, null, Modifier.size(18.dp), tint = content)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(ref.name, style = MaterialTheme.typography.bodyLarge, fontFamily = GeistMono, color = content, maxLines = 1, overflow = TextOverflow.Ellipsis)
            ref.worktreePath?.let {
                Text(
                    "Worktree · " + it.replace(Regex("^/(Users|home)/[^/]+"), "~"),
                    style = MaterialTheme.typography.bodySmall,
                    color = if (selected) content.copy(alpha = 0.75f) else MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        if (ref.current) Tag("current", selected)
        when {
            busy -> {
                Spacer(Modifier.width(8.dp))
                LoadingIndicator(Modifier.size(24.dp))
            }
            selected -> {
                Spacer(Modifier.width(8.dp))
                ZIcon(ZIcons.Check, "Selected", Modifier.size(20.dp), tint = content)
            }
        }
    }
}

@Composable
private fun Tag(text: String, selected: Boolean) {
    Surface(
        shape = RoundedCornerShape(50),
        color = if (selected) MaterialTheme.colorScheme.surfaceContainerLowest.copy(alpha = 0.6f) else chipContainer(),
        contentColor = MaterialTheme.colorScheme.onSurfaceVariant,
    ) {
        Text(text, style = MaterialTheme.typography.labelSmall, modifier = Modifier.padding(horizontal = 8.dp, vertical = 2.dp))
    }
}

/**
 * An open session's checkout, read-only: sessions never move refs (the
 * desktop shows these as labels), so this says where the agent works and
 * how to work elsewhere.
 */
@Composable
fun SessionBranchChip(branch: String?, cwd: String?, projectPath: String?) {
    var open by remember { mutableStateOf(false) }
    val worktree = cwd != null && projectPath != null && cwd.trimEnd('/') != projectPath.trimEnd('/')
    ContextChip(
        branch ?: "No branch",
        leading = { ZIcon(if (worktree) ZIcons.Project else ZIcons.Branch, null, Modifier.size(16.dp)) },
        onClick = { open = true },
    ) {
        AnchoredPopover(open, { open = false }) {
            Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text("Checkout", style = MaterialTheme.typography.titleSmallEmphasized)
                InfoLine(ZIcons.Branch, "Branch", branch ?: "Not reported yet", mono = branch != null)
                InfoLine(if (worktree) ZIcons.Project else ZIcons.Folder, if (worktree) "Worktree" else "Local checkout", cwd?.replace(Regex("^/(Users|home)/[^/]+"), "~") ?: "—", mono = true)
                Text(
                    "A session keeps the branch it started on. To work on another branch, start a new session and pick it there.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

@Composable
private fun InfoLine(icon: Int, title: String, value: String, mono: Boolean) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Box(
            Modifier.size(36.dp).clip(RoundedCornerShape(12.dp)).background(chipContainer()),
            contentAlignment = Alignment.Center,
        ) { ZIcon(icon, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant) }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(value, style = MaterialTheme.typography.bodyMedium, fontFamily = if (mono) GeistMono else null, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
    }
}
