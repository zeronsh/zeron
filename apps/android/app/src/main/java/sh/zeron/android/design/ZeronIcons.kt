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
    val Bell = R.drawable.zi_bell
    val Bot = R.drawable.zi_bot
    val Key = R.drawable.zi_key_minimalistic
    val Terminal = R.drawable.zi_terminal
    val Restart = R.drawable.zi_restart
    val Globe = R.drawable.zi_globe
    val AddCircle = R.drawable.zi_add_circle
    val Cloud = R.drawable.zi_cloud
    val Star = R.drawable.zi_star
    val StarFilled = R.drawable.zi_star_bold
    // Developer tools (desktop files panel, editor, browser, terminal).
    val FileTree = R.drawable.zi_file_tree
    val Save = R.drawable.zi_floppy_disk
    val Eye = R.drawable.zi_eye
    val EyeClosed = R.drawable.zi_eye_closed
    val Forward = R.drawable.zi_arrow_right
    val Code = R.drawable.zi_file_code
    val Markdown = R.drawable.zi_file_markdown
    val Keyboard = R.drawable.zi_keyboard
    val Play = R.drawable.zi_action_play
    val WrapText = R.drawable.zi_wrap_text
    val Expand = R.drawable.zi_expand_arrows
    val Collapse = R.drawable.zi_collapse_arrows
    val ChevronLeft = R.drawable.zi_alt_arrow_left
    val Window = R.drawable.zi_window_maximize
    val DocumentAdd = R.drawable.zi_document_add
    val Return = R.drawable.zi_return
    val HardDrive = R.drawable.zi_hard_drive
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
