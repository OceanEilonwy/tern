package xyz.hollingworth.tern.ui

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import xyz.hollingworth.tern.Store
import xyz.hollingworth.tern.core.Action
import xyz.hollingworth.tern.core.AddressField
import xyz.hollingworth.tern.core.ComposeText
import xyz.hollingworth.tern.core.ComposerView

/**
 * What is typed stays in the fields until it is sent or closed, so typing never waits on anything;
 * the engine takes it from there. This holds that text for the back gesture to hand over.
 */
object ComposerText {
    private var latest: (() -> ComposeText)? = null

    fun provide(text: (() -> ComposeText)?) {
        latest = text
    }

    fun current(view: ComposerView?): ComposeText = latest?.invoke() ?: view?.text ?: ComposeText("", "", "", "", "", 0u)
}

/** No more than this is read of a file; the engine refuses anything past its limit by the size. */
private const val READ_AT_MOST = 30 * 1024 * 1024

private fun readFile(context: Context, uri: Uri): Action {
    val resolver = context.contentResolver
    val name = runCatching {
        resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c -> if (c.moveToFirst()) c.getString(0) else null }
    }.getOrNull() ?: uri.lastPathSegment ?: "attachment"
    return runCatching {
        val data = resolver.openInputStream(uri)!!.use { it.readNBytes(READ_AT_MOST) }
        Action.ComposeAttach(name, resolver.getType(uri).orEmpty(), data)
    }.getOrElse { Action.ComposeAttachFailed(name) }
}

@Composable
fun ComposerScreen() {
    val composer by Store.composer.collectAsState()
    val view = lastOf(composer) ?: return
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var to by remember(view.revision) { mutableStateOf(view.text.to) }
    var cc by remember(view.revision) { mutableStateOf(view.text.cc) }
    var bcc by remember(view.revision) { mutableStateOf(view.text.bcc) }
    var subject by remember(view.revision) { mutableStateOf(view.text.subject) }
    var body by remember(view.revision) { mutableStateOf(view.text.body) }
    var from by remember(view.revision) { mutableIntStateOf(view.text.fromIndex.toInt()) }
    val text = { ComposeText(to, cc, bcc, subject, body, from.toUInt()) }
    // With someone to write to already, the cursor starts in the message.
    val bodyFocus = remember { FocusRequester() }
    LaunchedEffect(view.revision) { if (to.isNotEmpty()) runCatching { bodyFocus.requestFocus() } }
    ComposerText.provide(text)
    fun edited() {
        if (view.error != null) Store.send(Action.ComposeEdited)
    }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        scope.launch {
            uris.forEach { uri -> Store.send(withContext(Dispatchers.IO) { readFile(context, uri) }) }
        }
    }
    val colors = MaterialTheme.colorScheme
    Column(Modifier.fillMaxSize().navigationBarsPadding().imePadding()) {
        TopBar {
            IconAction(Icons.Close, "Close and keep as draft") { Store.send(Action.ComposeClose(text())) }
            Text(view.title, Modifier.weight(1f), style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            IconAction(Icons.Trash, "Discard") { Store.send(Action.ComposeDiscard(text())) }
            IconAction(Icons.Clip, "Attach files") { picker.launch(arrayOf("*/*")) }
            Button(
                { Store.send(Action.ComposeSend(text())) },
                Modifier.height(48.dp),
                enabled = !view.sending,
                colors = ButtonDefaults.buttonColors(containerColor = colors.primary, contentColor = colors.onPrimary),
            ) {
                if (view.sending) CircularProgressIndicator(Modifier.size(20.dp), colors.primary, strokeWidth = 2.dp)
                else Text("Send", style = MaterialTheme.typography.titleMedium)
            }
            Spacer(Modifier.width(8.dp))
        }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
            FromRow(view, from) { from = it }
            AddressRow("To", to, view.badTo, focusFirst = to.isEmpty(), trailing = {
                if (!view.showCc) TextButton({ Store.send(Action.ComposeShowCc) }) { Text("Cc/Bcc", color = colors.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium) }
            }) {
                to = it
                edited()
                Store.send(Action.ComposeSuggest(AddressField.TO, it))
            }
            Suggestions(view, AddressField.TO, text)
            if (view.showCc) {
                AddressRow("Cc", cc, view.badCc) {
                    cc = it
                    edited()
                    Store.send(Action.ComposeSuggest(AddressField.CC, it))
                }
                Suggestions(view, AddressField.CC, text)
                AddressRow("Bcc", bcc, view.badBcc) {
                    bcc = it
                    edited()
                    Store.send(Action.ComposeSuggest(AddressField.BCC, it))
                }
                Suggestions(view, AddressField.BCC, text)
            }
            FieldRow("Subject") {
                Line(subject, { subject = it }, Modifier.weight(1f), "Subject", capitalize = true)
            }
            Box(Modifier.fillMaxWidth().height(1.dp).background(colors.outlineVariant))
            view.error?.let { error ->
                Text(error, Modifier.padding(horizontal = 16.dp, vertical = 8.dp), style = MaterialTheme.typography.bodyMedium, color = colors.error)
            }
            if (view.attachments.isNotEmpty()) Attachments(view)
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                view.pickedText?.let { lines -> PickedQuote(view.pickedWho.orEmpty(), lines) }
                BasicTextField(
                    body,
                    {
                        body = it
                        edited()
                    },
                    Modifier.fillMaxWidth().heightIn(min = 160.dp).focusRequester(bodyFocus).semantics { contentDescription = "Message" },
                    textStyle = MaterialTheme.typography.bodyLarge.copy(color = colors.onSurface),
                    cursorBrush = SolidColor(colors.primary),
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
                )
                view.wholeLabel?.let { label -> WholeQuote(label, view.wholeText.orEmpty(), view.wholeIncluded) }
            }
        }
    }
}

@Composable
private fun FieldRow(label: String, content: @Composable androidx.compose.foundation.layout.RowScope.() -> Unit) {
    val colors = MaterialTheme.colorScheme
    Column {
        Box(Modifier.fillMaxWidth().height(1.dp).background(colors.outlineVariant))
        Row(Modifier.fillMaxWidth().heightIn(min = 52.dp).padding(start = 16.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(label, Modifier.width(56.dp), colors.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium)
            content()
        }
    }
}

@Composable
private fun Line(value: String, onChange: (String) -> Unit, modifier: Modifier, description: String, email: Boolean = false, capitalize: Boolean = false, bad: Boolean = false, focus: FocusRequester? = null) {
    val colors = MaterialTheme.colorScheme
    var m = modifier.semantics { contentDescription = description }
    if (focus != null) m = m.focusRequester(focus)
    BasicTextField(
        value,
        onChange,
        m,
        textStyle = MaterialTheme.typography.bodyLarge.copy(color = if (bad) colors.error else colors.onSurface),
        singleLine = true,
        cursorBrush = SolidColor(colors.primary),
        keyboardOptions = KeyboardOptions(
            keyboardType = if (email) KeyboardType.Email else KeyboardType.Text,
            capitalization = if (capitalize) KeyboardCapitalization.Sentences else KeyboardCapitalization.None,
        ),
    )
}

@Composable
private fun AddressRow(label: String, value: String, bad: Boolean, focusFirst: Boolean = false, trailing: @Composable () -> Unit = {}, onChange: (String) -> Unit) {
    val focus = remember { FocusRequester() }
    if (focusFirst) LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
    FieldRow(label) {
        Line(value, onChange, Modifier.weight(1f), label, email = true, bad = bad, focus = focus)
        trailing()
    }
}

@Composable
private fun FromRow(view: ComposerView, from: Int, onPick: (Int) -> Unit) {
    val colors = MaterialTheme.colorScheme
    var open by remember { mutableStateOf(false) }
    val many = view.addresses.size > 1
    FieldRow("From") {
        Box {
            Row(
                Modifier.background(colors.surfaceContainer, RoundedCornerShape(12.dp)).clickable(enabled = many) { open = true }.height(40.dp).padding(start = 12.dp, end = if (many) 8.dp else 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(view.addresses.getOrNull(from).orEmpty(), style = MaterialTheme.typography.labelLarge, maxLines = 1)
                if (many) Glyph(Icons.Down, null, size = 18.dp, modifier = Modifier.padding(start = 6.dp))
            }
            DropdownMenu(open, { open = false }) {
                view.addresses.forEachIndexed { i, address ->
                    DropdownMenuItem({ Text(address) }, {
                        open = false
                        onPick(i)
                    })
                }
            }
        }
    }
}

@Composable
private fun Suggestions(view: ComposerView, field: AddressField, text: () -> ComposeText) {
    if (view.suggestField != field) return
    val colors = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth().background(colors.surfaceContainerHigh)) {
        view.suggestions.forEachIndexed { i, suggestion ->
            Column(Modifier.fillMaxWidth().clickable { Store.send(Action.ComposeTakeSuggestion(text(), field, i.toUInt())) }.heightIn(min = 48.dp).padding(horizontal = 16.dp, vertical = 6.dp)) {
                Text(suggestion.name.ifEmpty { suggestion.address }, style = MaterialTheme.typography.labelLarge, maxLines = 1)
                if (suggestion.name.isNotEmpty()) Text(suggestion.address, style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant, maxLines = 1)
            }
        }
    }
}

@Composable
private fun Attachments(view: ComposerView) {
    val colors = MaterialTheme.colorScheme
    Column(Modifier.padding(start = 12.dp, end = 12.dp, top = 8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        view.attachments.forEachIndexed { i, attachment ->
            Row(Modifier.fillMaxWidth().background(colors.surfaceContainer, RoundedCornerShape(12.dp)).padding(start = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                Glyph(Icons.Clip, null, colors.onSurfaceVariant, 18.dp)
                Column(Modifier.weight(1f).padding(horizontal = 10.dp, vertical = 6.dp)) {
                    Text(attachment.name, style = MaterialTheme.typography.labelLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    if (attachment.size.isNotEmpty()) Text(attachment.size, style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
                }
                if (attachment.pending) CircularProgressIndicator(Modifier.padding(12.dp).size(20.dp), colors.primary, strokeWidth = 2.dp)
                else IconAction(Icons.Close, "Remove ${attachment.name}") { Store.send(Action.ComposeRemoveAttachment(i.toUInt())) }
            }
        }
    }
}

@Composable
private fun PickedQuote(who: String, lines: String) {
    val colors = MaterialTheme.colorScheme
    Row(Modifier.fillMaxWidth().background(colors.primaryContainer.copy(alpha = 0.5f), RoundedCornerShape(12.dp)).padding(start = 14.dp), verticalAlignment = Alignment.Top) {
        Column(Modifier.weight(1f).padding(top = 10.dp, bottom = 12.dp)) {
            Text(who, style = MaterialTheme.typography.bodySmall, color = colors.onPrimaryContainer)
            Text(lines, Modifier.padding(top = 4.dp), style = MaterialTheme.typography.bodyLarge)
        }
        IconAction(Icons.Close, "Leave out the quote") { Store.send(Action.ComposeDropPicked) }
    }
}

/** The earlier mail: dashed while left out and filled once it goes, as on desktop. */
@Composable
private fun WholeQuote(label: String, whole: String, included: Boolean) {
    val colors = MaterialTheme.colorScheme
    var look by remember { mutableStateOf(false) }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        val edge = if (included) Modifier.background(colors.primaryContainer.copy(alpha = 0.6f), RoundedCornerShape(24.dp)) else Modifier.border(1.dp, colors.outline, RoundedCornerShape(24.dp))
        Row(edge.height(48.dp), verticalAlignment = Alignment.CenterVertically) {
            TextButton({ look = !look }, Modifier.semantics { contentDescription = "Look at $label" }) { Text("•••", color = colors.onSurface, style = MaterialTheme.typography.labelLarge) }
            TextButton({ Store.send(Action.ComposeIncludeWhole(!included)) }) {
                Text(if (included) label else "+ Quote $label", color = colors.onSurface, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            if (included) IconAction(Icons.Close, "Leave out $label") { Store.send(Action.ComposeIncludeWhole(false)) }
        }
        if (look) {
            Text(
                whole,
                Modifier.fillMaxWidth().background(colors.surfaceContainer, RoundedCornerShape(12.dp)).padding(horizontal = 14.dp, vertical = 12.dp),
                style = MaterialTheme.typography.bodyMedium,
                color = if (included) colors.onSurface else colors.onSurfaceVariant,
            )
        }
    }
}
