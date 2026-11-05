package xyz.hollingworth.tern.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CheckboxDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TriStateCheckbox
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.state.ToggleableState
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import xyz.hollingworth.tern.Store
import xyz.hollingworth.tern.core.Action
import xyz.hollingworth.tern.core.CleanupChoice
import xyz.hollingworth.tern.core.CleanupPart
import xyz.hollingworth.tern.core.LabelChoice
import xyz.hollingworth.tern.core.SheetView

/** Whatever the engine says is open over the screen. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun Sheets() {
    val sheet by Store.sheet.collectAsState()
    when (val view = sheet) {
        null -> {}
        SheetView.Discard -> AlertDialog(
            onDismissRequest = { Store.send(Action.CloseSheet) },
            title = { Text("Discard message?") },
            text = { Text("The message and its draft will be deleted.") },
            confirmButton = { TextButton({ Store.send(Action.ComposeDiscardConfirmed) }) { Text("Discard", color = MaterialTheme.colorScheme.error) } },
            dismissButton = { TextButton({ Store.send(Action.CloseSheet) }) { Text("Cancel") } },
            containerColor = MaterialTheme.colorScheme.surfaceContainerHigh,
        )
        else -> ModalBottomSheet(
            onDismissRequest = { Store.send(Action.CloseSheet) },
            sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
            containerColor = MaterialTheme.colorScheme.surfaceContainerHigh,
            scrimColor = MaterialTheme.colorScheme.background.copy(alpha = 0.6f),
        ) {
            Column(Modifier.navigationBarsPadding().imePadding()) {
                when (view) {
                    is SheetView.Labels -> Labels(view.subject, view.choices)
                    is SheetView.Spam -> Spam(view)
                    is SheetView.Cleanup -> Cleanup(view.total, view.choices)
                    else -> {}
                }
            }
        }
    }
}

@Composable
private fun Labels(subject: String, choices: List<LabelChoice>) {
    val colors = MaterialTheme.colorScheme
    var naming by remember { mutableStateOf(false) }
    var name by remember { mutableStateOf("") }
    Text("Labels", Modifier.padding(horizontal = 24.dp), style = MaterialTheme.typography.titleLarge)
    Text(subject, Modifier.padding(start = 24.dp, end = 24.dp, bottom = 8.dp), style = MaterialTheme.typography.bodyMedium, color = colors.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
    LazyColumn(Modifier.heightIn(max = 360.dp)) {
        items(choices, key = { it.id }) { choice ->
            Row(
                Modifier.fillMaxWidth().clickable(role = Role.Checkbox) { Store.send(Action.ToggleLabel(choice.id)) }.height(56.dp).padding(start = 24.dp, end = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(choice.name, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge, maxLines = 1)
                TriStateCheckbox(
                    when {
                        choice.on -> ToggleableState.On
                        choice.mixed -> ToggleableState.Indeterminate
                        else -> ToggleableState.Off
                    },
                    onClick = null,
                    colors = CheckboxDefaults.colors(checkedColor = colors.primary, checkmarkColor = colors.onPrimary),
                    modifier = Modifier.size(48.dp),
                )
            }
        }
    }
    if (naming) {
        OutlinedTextField(
            name,
            { name = it },
            Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 8.dp),
            label = { Text("New label name") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = {
                Store.send(Action.CreateLabel(name))
                name = ""
                naming = false
            }),
        )
    } else {
        Row(
            Modifier.fillMaxWidth().clickable(role = Role.Button) { naming = true }.height(56.dp).padding(horizontal = 24.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Glyph(Icons.Plus, null, Accent)
            Text("New label", color = Accent, style = MaterialTheme.typography.labelLarge)
        }
    }
    Spacer(Modifier.height(16.dp))
}

@Composable
private fun Spam(view: SheetView.Spam) {
    val colors = MaterialTheme.colorScheme
    var field by remember(view.revision) { mutableStateOf(TextFieldValue(view.line, TextRange(view.line.length))) }
    Text("Move to spam", Modifier.padding(horizontal = 24.dp), style = MaterialTheme.typography.titleLarge)
    Text("Mail like this", Modifier.padding(start = 24.dp, end = 24.dp, top = 8.dp, bottom = 4.dp), style = MaterialTheme.typography.titleSmall)
    OutlinedTextField(
        field,
        {
            val changed = it.text != field.text
            field = it
            if (changed) Store.send(Action.SpamLine(it.text))
        },
        Modifier.fillMaxWidth().padding(horizontal = 24.dp),
        textStyle = MaterialTheme.typography.bodyMedium.copy(fontFamily = Mono),
        singleLine = true,
        isError = view.problem != null,
        supportingText = view.problem?.let { { Text(it) } },
    )
    Row(Modifier.padding(horizontal = 24.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        view.choices.forEach { choice -> TextButton({ Store.send(Action.SpamUse(choice.value)) }) { Text(choice.label, color = Accent, style = MaterialTheme.typography.labelLarge) } }
    }
    Row(Modifier.padding(start = 24.dp, end = 12.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(view.count, Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
        val all = view.rows.isNotEmpty() && view.rows.all { it.picked }
        TextButton({ Store.send(Action.SpamSelectAll(!all)) }) { Text(if (all) "Select none" else "Select all", color = Accent, style = MaterialTheme.typography.bodyMedium) }
    }
    LazyColumn(Modifier.heightIn(max = 320.dp)) {
        items(view.rows, key = { it.id.list + it.id.element }) { row ->
            Row(
                Modifier.fillMaxWidth().clickable(role = Role.Checkbox) { Store.send(Action.SpamToggle(row.id)) }.heightIn(min = 56.dp).padding(start = 12.dp, end = 24.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Checkbox(row.picked, null, Modifier.size(48.dp), colors = CheckboxDefaults.colors(checkedColor = colors.primary, checkmarkColor = colors.onPrimary))
                Column(Modifier.weight(1f)) {
                    Text(row.who, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(row.subject, style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                Text(row.date, Modifier.padding(start = 8.dp), style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
            }
        }
    }
    Row(Modifier.padding(horizontal = 24.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text("Always move mail like this to spam", style = MaterialTheme.typography.bodyMedium)
            Text("New mail this line catches goes straight there", style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
        }
        Switch(view.keepRule, { Store.send(Action.SpamKeepRule(it)) })
    }
    Button({ Store.send(Action.SpamConfirm) }, Modifier.fillMaxWidth().padding(start = 24.dp, end = 24.dp, bottom = 16.dp).height(48.dp)) {
        Text(view.confirm, style = MaterialTheme.typography.labelLarge)
    }
}

@Composable
private fun Cleanup(total: String, choices: List<CleanupChoice>) {
    val colors = MaterialTheme.colorScheme
    var picked by remember { mutableStateOf(setOf(CleanupPart.OLDER)) }
    Text("Clean up", Modifier.padding(horizontal = 24.dp), style = MaterialTheme.typography.titleLarge)
    Text("Tern takes $total on this phone.", Modifier.padding(start = 24.dp, end = 24.dp, bottom = 8.dp), style = MaterialTheme.typography.bodyMedium, color = colors.onSurfaceVariant)
    choices.forEach { choice ->
        Row(
            Modifier.fillMaxWidth().clickable(role = Role.Checkbox) { picked = if (choice.part in picked) picked - choice.part else picked + choice.part }.heightIn(min = 64.dp).padding(start = 12.dp, end = 24.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Checkbox(choice.part in picked, null, Modifier.size(48.dp), colors = CheckboxDefaults.colors(checkedColor = colors.primary, checkmarkColor = colors.onPrimary))
            Column(Modifier.weight(1f)) {
                Text(choice.title, style = MaterialTheme.typography.bodyLarge, color = if (choice.part == CleanupPart.ALL) colors.error else colors.onSurface)
                Text(choice.detail, style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
            }
            Text(choice.size, Modifier.padding(start = 8.dp), style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
        }
    }
    Box(Modifier.padding(24.dp, 8.dp, 24.dp, 16.dp)) {
        Button(
            {
                Store.send(Action.RunCleanup(CleanupPart.OLDER in picked, CleanupPart.VECTORS in picked, CleanupPart.MODEL in picked, CleanupPart.ALL in picked))
            },
            Modifier.fillMaxWidth().height(48.dp),
            enabled = picked.isNotEmpty(),
        ) { Text("Clean up", style = MaterialTheme.typography.labelLarge) }
    }
}
