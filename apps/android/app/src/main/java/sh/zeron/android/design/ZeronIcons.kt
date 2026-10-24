package sh.zeron.android.design

import androidx.annotation.DrawableRes
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalContentColor
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import sh.zeron.android.R

/**
 * Zeron's icon set — the same glyphs the desktop (crates/ui/assets/icons,
 * Solar Linear + Zeron's hand-drawn additions) and iOS draw, compiled to
 * VectorDrawables at build time by scripts/android/svg2vd.py.
 */
object ZIcons {
    val NewSession = R.drawable.zi_pen_new_square
    val Search = R.drawable.zi_magnifer
    val TabSessions = R.drawable.zi_tab_chat
    val TabSettings = R.drawable.zi_tab_settings
    val Back = R.drawable.zi_arrow_left
    val More = R.drawable.zi_more_horizontal
    val Close = R.drawable.zi_close
    val Plus = R.drawable.zi_plus
    val Attach = R.drawable.zi_paperclip
    val Send = R.drawable.zi_arrow_up
    val Stop = R.drawable.zi_stop
    val Check = R.drawable.zi_check
    val ChevronDown = R.drawable.zi_alt_arrow_down
    val ChevronUp = R.drawable.zi_alt_arrow_up
    val ChevronRight = R.drawable.zi_alt_arrow_right
    val ArrowDown = R.drawable.zi_arrow_down
    val Branch = R.drawable.zi_git_branch
    val PullRequest = R.drawable.zi_pull_request
    val Effort = R.drawable.zi_tuning
    val Queue = R.drawable.zi_clock_circle
    val Context = R.drawable.zi_widget
    val Warning = R.drawable.zi_danger_triangle
    val Pin = R.drawable.zi_pin
    val Archive = R.drawable.zi_archive_minimalistic
    val Unarchive = R.drawable.zi_archive_up_minimalistic
    val Rename = R.drawable.zi_pen
    val Copy = R.drawable.zi_copy
    val Delete = R.drawable.zi_trash_bin_minimalistic
    val Home = R.drawable.zi_home
    val Folder = R.drawable.zi_folder
    val Project = R.drawable.zi_folder_with_files
    val Laptop = R.drawable.zi_laptop
    val Monitor = R.drawable.zi_monitor
    val Phone = R.drawable.zi_smartphone
    val Server = R.drawable.zi_remote_server
    val Sun = R.drawable.zi_sun
    val Moon = R.drawable.zi_moon
    val Magic = R.drawable.zi_magic_stick_3
    val Info = R.drawable.zi_info_circle
    val Logout = R.drawable.zi_logout_2
    val Offline = R.drawable.zi_wifi_off
    val Chat = R.drawable.zi_chat_round_line
    val Refresh = R.drawable.zi_refresh
    val Text = R.drawable.zi_document
    val Image = R.drawable.zi_file_image
    val Link = R.drawable.zi_arrow_up_right
    val Share = R.drawable.zi_share
    val Camera = R.drawable.zi_camera
    val Gallery = R.drawable.zi_gallery
    val File = R.drawable.zi_document_add
    val Bell = R.drawable.zi_bell
    val Mic = R.drawable.zi_microphone
    val MicOff = R.drawable.zi_microphone_off
    val HangUp = R.drawable.zi_phone_hang_up
    val Speaker = R.drawable.zi_volume_loud
    val Collapse = R.drawable.zi_collapse_arrows
    val Sort = R.drawable.zi_sort_vertical
    val FolderTree = R.drawable.zi_folder_tree
    val Worktree = R.drawable.zi_worktree
    val Restart = R.drawable.zi_restart
    val Command = R.drawable.zi_command
    val Settings = R.drawable.zi_settings
    val Terminal = R.drawable.zi_terminal
    val Fork = R.drawable.zi_fork
    val Bot = R.drawable.zi_bot
    val SideChats = R.drawable.zi_chats
    val Companions = R.drawable.zi_layers
    val Keyboard = R.drawable.zi_keyboard
    val KeyboardHide = R.drawable.zi_keyboard_hide
    val Paste = R.drawable.zi_clipboard
    val ArrowUp = R.drawable.zi_alt_arrow_up
    val ArrowLeft = R.drawable.zi_alt_arrow_left
    val ArrowRight = R.drawable.zi_alt_arrow_right
    val Minus = R.drawable.zi_minus
    val Cloud = R.drawable.zi_cloud
    val Wallet = R.drawable.zi_wallet
    val Key = R.drawable.zi_key_minimalistic
    val Globe = R.drawable.zi_global
    val GitHub = R.drawable.zi_github
    val Zeron = R.drawable.zi_zeron
}

@Composable
fun ZIcon(
    @DrawableRes icon: Int,
    contentDescription: String?,
    modifier: Modifier = Modifier,
    tint: Color = LocalContentColor.current,
) {
    Icon(painterResource(icon), contentDescription, modifier, tint)
}
