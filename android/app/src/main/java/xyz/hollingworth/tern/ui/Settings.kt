package xyz.hollingworth.tern.ui

import android.Manifest
import android.app.NotificationManager
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.compose.ui.unit.dp
import xyz.hollingworth.tern.Store
import com.tern.engine.Action
import com.tern.engine.ChoiceView

@Composable
private fun Group(title: String, content: @Composable () -> Unit) {
    Column {
        SectionTitle(title)
        Column(Modifier.fillMaxWidth().border(1.dp, MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(12.dp))) { content() }
    }
}

@Composable
private fun SettingRow(title: String, line: String?, divider: Boolean = true, onClick: (() -> Unit)? = null, trailing: @Composable RowScope.() -> Unit = {}) {
    val colors = MaterialTheme.colorScheme
    var modifier = Modifier.fillMaxWidth()
    if (onClick != null) modifier = modifier.clickable(onClick = onClick)
    if (divider) Box(Modifier.fillMaxWidth().height(1.dp).background(colors.outlineVariant))
    Row(modifier.heightIn(min = 56.dp).padding(start = 16.dp, end = 12.dp, top = 8.dp, bottom = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            if (!line.isNullOrEmpty()) Text(line, style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
        }
        trailing()
    }
}

@Composable
private fun TernSwitch(on: Boolean, description: String, change: (Boolean) -> Unit) {
    val colors = MaterialTheme.colorScheme
    Switch(
        on,
        change,
        colors = SwitchDefaults.colors(
            checkedTrackColor = colors.primary,
            checkedThumbColor = colors.onPrimary,
            uncheckedTrackColor = colors.background,
            uncheckedBorderColor = colors.outline,
            uncheckedThumbColor = colors.outline,
        ),
    )
}

@Composable
private fun RowAction(label: String, onClick: () -> Unit) {
    FilledTonalButton(
        onClick,
        Modifier.height(48.dp),
        colors = ButtonDefaults.filledTonalButtonColors(containerColor = MaterialTheme.colorScheme.surfaceContainer, contentColor = MaterialTheme.colorScheme.onSurface),
    ) { Text(label, style = MaterialTheme.typography.labelLarge) }
}

@Composable
private fun ChoiceRow(title: String, line: String, choices: List<ChoiceView>, pick: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        SettingRow(title, line, onClick = { open = true }) { Glyph(Icons.Down, null, MaterialTheme.colorScheme.onSurfaceVariant, 20.dp) }
        DropdownMenu(open, { open = false }) {
            choices.forEach { choice ->
                DropdownMenuItem(
                    { Text(choice.label) },
                    {
                        open = false
                        pick(choice.value)
                    },
                    trailingIcon = if (choice.selected) ({ Glyph(Icons.Check, "Chosen", size = 18.dp) }) else null,
                )
            }
        }
    }
}

/** The switch is the engine's setting; whether the system lets Tern say anything is the system's. */
@Composable
private fun NotificationsRow(on: Boolean, line: String) {
    val context = LocalContext.current
    fun allowed() = context.getSystemService(NotificationManager::class.java).areNotificationsEnabled()
    var asked by remember { mutableIntStateOf(0) }
    val blocked = remember(asked, on) { on && !allowed() }
    val systemSettings = { runCatching { context.startActivity(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)) } }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        asked++
        if (granted) Store.send(Action.SetNotifications(true)) else systemSettings()
    }
    SettingRow(
        "Notifications",
        if (blocked) "Blocked in Android’s settings. Tap to change." else line,
        onClick = if (blocked) ({ systemSettings() }) else null,
    ) {
        TernSwitch(on, "Notifications") { wanted ->
            when {
                !wanted -> Store.send(Action.SetNotifications(false))
                allowed() -> Store.send(Action.SetNotifications(true))
                else -> ask.launch(Manifest.permission.POST_NOTIFICATIONS)
            }
        }
    }
}

/**
 * Android has no default-mail-app setting an app can ask for: the default is whichever app it sends
 * a mailto link to, chosen with "Always" in its picker. So this asks the system who that is.
 */
@Composable
private fun DefaultMailRow() {
    val context = LocalContext.current
    // SENDTO is how apps send mail, and only mail apps take it. "Always" on it holds for any address.
    val picker = remember { Intent(Intent.ACTION_SENDTO, Uri.parse("mailto:")) }
    val probe = remember { Intent(Intent.ACTION_SENDTO, Uri.parse("mailto:someone@example.org")) }
    var asked by remember { mutableIntStateOf(0) }
    val owner = LocalLifecycleOwner.current
    // Coming back from the picker or the system's settings, ask again.
    DisposableEffect(owner) {
        val watcher = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_RESUME) asked++ }
        owner.lifecycle.addObserver(watcher)
        onDispose { owner.lifecycle.removeObserver(watcher) }
    }
    val current = remember(asked) { context.packageManager.resolveActivity(probe, PackageManager.MATCH_DEFAULT_ONLY)?.activityInfo }
    // With no default yet, the system itself answers: its picker.
    val chosen = current?.packageName?.takeIf { it != "android" && current.name?.contains("Resolver") != true }
    val ours = chosen == context.packageName
    val other = chosen?.takeIf { !ours }
    val otherName = remember(other) { other?.let { runCatching { context.packageManager.getApplicationLabel(context.packageManager.getApplicationInfo(it, 0)).toString() }.getOrNull() ?: it } }
    SettingRow(
        "Default mail app",
        when {
            ours -> "Email links open in Tern"
            other != null -> "Email links open in $otherName. Clear its defaults to choose Tern."
            else -> "Choose Tern, then Always"
        },
        divider = false,
    ) {
        if (!ours) {
            RowAction(if (other != null) "Change" else "Use Tern") {
                val next = if (other != null) Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:$other")) else picker
                runCatching { context.startActivity(next) }
            }
        }
    }
}

@Composable
fun SettingsScreen() {
    val settings by Store.settings.collectAsState()
    val view = settings ?: return
    Column(Modifier.fillMaxSize()) {
        BackBar("Settings", onBack = { Store.send(Action.Back) })
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).navigationBarsPadding().padding(start = 12.dp, end = 12.dp, bottom = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Group("Tern on this phone") {
                DefaultMailRow()
                SettingRow("Pure black", "Dark theme uses true black on OLED screens") { TernSwitch(view.pureBlack, "Pure black") { Store.send(Action.SetPureBlack(it)) } }
                SettingRow("Match wallpaper colours", "Off keeps Tern’s blue") { TernSwitch(view.matchWallpaper, "Match wallpaper colours") { Store.send(Action.SetMatchWallpaper(it)) } }
            }
            Group("In the background") {
                ChoiceRowNoDivider("Check for new mail", view.backgroundLine, view.background) { Store.send(Action.SetBackground(it)) }
                NotificationsRow(view.notifications, view.notificationsLine)
            }
            Group("Offline mail") {
                Column {
                    Box { ChoiceRowNoDivider("Keep ready", view.keepReadyLine, view.keepReady) { Store.send(Action.SetKeepReady(it)) } }
                    SettingRow("Forget mail on sign out", "Signing in again downloads it afresh") { TernSwitch(view.forgetOnSignOut, "Forget mail on sign out") { Store.send(Action.SetForget(it)) } }
                    ChoiceRow("Offer cleanup past", view.cleanupLimitLine, view.cleanupLimits) { Store.send(Action.SetCleanupLimit(it)) }
                    SettingRow("Storage", view.storageLine) { RowAction("Clean up…") { Store.send(Action.OpenCleanup) } }
                }
            }
            Group("Search") {
                SettingRow("Search by meaning", view.semanticLine, divider = false) { TernSwitch(view.semantic, "Search by meaning") { Store.send(Action.SetSemantic(it)) } }
            }
            Group("Rules") {
                SettingRow("Mail rules", view.rulesLine, divider = false, onClick = { Store.send(Action.OpenRules) }) { Glyph(Icons.Next, null, MaterialTheme.colorScheme.onSurfaceVariant, 20.dp) }
            }
            Group("Account") {
                SettingRow(view.address, null, divider = false) { RowAction("Sign out") { Store.send(Action.SignOut) } }
            }
            Spacer(Modifier.height(8.dp))
            Text("Unofficial client for Tuta mail. Not affiliated with Tuta GmbH.", Modifier.padding(horizontal = 4.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
private fun ChoiceRowNoDivider(title: String, line: String, choices: List<ChoiceView>, pick: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        SettingRow(title, line, divider = false, onClick = { open = true }) { Glyph(Icons.Down, null, MaterialTheme.colorScheme.onSurfaceVariant, 20.dp) }
        DropdownMenu(open, { open = false }) {
            choices.forEach { choice ->
                DropdownMenuItem(
                    { Text(choice.label) },
                    {
                        open = false
                        pick(choice.value)
                    },
                    trailingIcon = if (choice.selected) ({ Glyph(Icons.Check, "Chosen", size = 18.dp) }) else null,
                )
            }
        }
    }
}
