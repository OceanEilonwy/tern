package xyz.hollingworth.tern.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.OffsetMapping
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.input.TransformedText
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import xyz.hollingworth.tern.Store
import com.tern.engine.Action
import com.tern.engine.LinePart
import com.tern.engine.LineSpan
import com.tern.engine.PreviewTone
import com.tern.engine.RuleDoes
import com.tern.engine.RuleEditorView
import com.tern.engine.RulePiece

@Composable
fun RulesScreen() {
    val rules by Store.rules.collectAsState()
    val view = rules ?: return
    val colors = MaterialTheme.colorScheme
    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize()) {
            BackBar("Mail rules", onBack = { Store.send(Action.Back) })
            Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 112.dp)) {
                Text("Rules sort new mail in the Inbox as it arrives.", Modifier.padding(start = 16.dp, end = 16.dp, bottom = 12.dp), style = MaterialTheme.typography.bodyMedium, color = colors.onSurfaceVariant)
                if (view.rows.isNotEmpty()) {
                    Column(Modifier.padding(horizontal = 12.dp).fillMaxWidth().border(1.dp, colors.outlineVariant, RoundedCornerShape(12.dp))) {
                        view.rows.forEachIndexed { i, rule ->
                            if (i > 0) Box(Modifier.fillMaxWidth().height(1.dp).background(colors.outlineVariant))
                            Row(
                                Modifier.fillMaxWidth().clickable { Store.send(Action.OpenRule(rule.index)) }.heightIn(min = 72.dp).padding(start = 16.dp, end = 12.dp, top = 10.dp, bottom = 10.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                                    Text(rule.`when`, fontFamily = Mono, style = MaterialTheme.typography.bodyMedium)
                                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                                        if (rule.broken) Glyph(Icons.Spam, null, colors.error, 14.dp)
                                        Text(rule.does, style = MaterialTheme.typography.bodySmall, color = if (rule.broken) colors.error else colors.onSurfaceVariant)
                                    }
                                }
                                Glyph(Icons.Next, null, colors.onSurfaceVariant, 20.dp)
                            }
                        }
                    }
                }
            }
        }
        ExtendedFloatingActionButton(
            onClick = { Store.send(Action.OpenRule(null)) },
            modifier = Modifier.align(Alignment.BottomEnd).navigationBarsPadding().padding(16.dp).semantics { contentDescription = "Add rule" },
            containerColor = colors.primary,
            contentColor = colors.onPrimary,
            shape = RoundedCornerShape(16.dp),
            icon = { Glyph(Icons.Plus, null, colors.onPrimary) },
            text = { Text("Add rule", style = MaterialTheme.typography.titleMedium) },
        )
    }
}

/** The line in colour, as the engine read it: spans count chars, and the text is all there is. */
private fun colouredLine(text: String, spans: List<LineSpan>, muted: Color): AnnotatedString = buildAnnotatedString {
    append(text)
    val indexOfChar = IntArray(text.codePointCount(0, text.length) + 1)
    var at = 0
    var char = 0
    while (at < text.length) {
        indexOfChar[char++] = at
        at += Character.charCount(text.codePointAt(at))
    }
    indexOfChar[char] = text.length
    for (span in spans) {
        val start = indexOfChar.getOrNull(span.start.toInt()) ?: continue
        val end = indexOfChar.getOrNull(span.end.toInt()) ?: continue
        val style = when (span.part) {
            LinePart.FROM -> SpanStyle(color = RuleFrom, fontWeight = FontWeight.Bold)
            LinePart.TO -> SpanStyle(color = RuleTo, fontWeight = FontWeight.Bold)
            LinePart.SUBJECT -> SpanStyle(color = RuleSubject, fontWeight = FontWeight.Bold)
            LinePart.OPERATOR -> SpanStyle(color = muted, fontStyle = FontStyle.Italic)
            LinePart.BRACKET -> SpanStyle(color = muted)
            LinePart.VALUE -> continue
        }
        addStyle(style, start, end)
    }
}

@Composable
fun RuleEditorScreen() {
    val editor by Store.ruleEditor.collectAsState()
    val view = lastOf(editor) ?: return
    val colors = MaterialTheme.colorScheme
    var field by remember(view.revision) { mutableStateOf(TextFieldValue(view.line, TextRange(lineOffset(view.line, view.cursor.toInt())))) }
    val focus = remember { FocusRequester() }
    // Opening the page and each button under the line leave the cursor in the line, ready to type.
    LaunchedEffect(view.revision) { runCatching { focus.requestFocus() } }
    fun chars(offset: Int) = field.text.codePointCount(0, offset.coerceIn(0, field.text.length)).toUInt()
    Column(Modifier.fillMaxSize().imePadding()) {
        TopBar {
            IconAction(Icons.Back, "Back") { Store.send(Action.Back) }
            Text(if (view.editing) "Edit rule" else "New rule", Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
            Button({ Store.send(Action.RuleSave) }, Modifier.height(48.dp), enabled = view.canSave) { Text("Save", style = MaterialTheme.typography.titleMedium) }
            Spacer(Modifier.width(8.dp))
        }
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).navigationBarsPadding().padding(start = 12.dp, end = 12.dp, bottom = 16.dp)) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("Which mail", Modifier.weight(1f), style = MaterialTheme.typography.titleSmall)
                var help by remember { mutableStateOf(false) }
                TextButton({ help = !help }) { Text("How to write a rule", color = Accent, style = MaterialTheme.typography.bodyMedium) }
                if (help) {
                    DropdownMenu(true, { help = false }) {
                        Text(
                            "from: to: subject: look at a mail’s sender, first recipient and subject. @example.org matches a whole domain. Join them with and, or, not, and brackets. Put text with spaces in quotes.",
                            Modifier.padding(horizontal = 16.dp, vertical = 8.dp).width(280.dp),
                            style = MaterialTheme.typography.bodyMedium,
                        )
                    }
                }
            }
            val muted = colors.onSurfaceVariant
            BasicTextField(
                field,
                {
                    val changed = it.text != field.text
                    field = it
                    if (changed) Store.send(Action.RuleLine(it.text))
                },
                Modifier.fillMaxWidth().border(2.dp, colors.primary, RoundedCornerShape(12.dp)).padding(horizontal = 16.dp, vertical = 14.dp).focusRequester(focus).semantics { contentDescription = "The rule" },
                textStyle = MaterialTheme.typography.bodyMedium.copy(color = colors.onSurface, fontFamily = Mono),
                cursorBrush = SolidColor(colors.primary),
                visualTransformation = VisualTransformation { text ->
                    TransformedText(colouredLine(text.text, if (text.text == view.line) view.spans else emptyList(), muted), OffsetMapping.Identity)
                },
            )
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                listOf(
                    Triple("+ From", RulePiece.FROM, RuleFrom),
                    Triple("+ To", RulePiece.TO, RuleTo),
                    Triple("+ Subject", RulePiece.SUBJECT, RuleSubject),
                    Triple("and", RulePiece.AND, colors.onSurface),
                    Triple("or", RulePiece.OR, colors.onSurface),
                    Triple("not", RulePiece.NOT, colors.onSurface),
                    Triple("( … )", RulePiece.BRACKETS, colors.onSurface),
                ).forEach { (label, piece, tone) ->
                    Box(
                        Modifier.height(48.dp).clickable(role = Role.Button) {
                            Store.send(Action.RulePut(piece, chars(field.selection.min), chars(field.selection.max)))
                        },
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(label, Modifier.background(tone.copy(alpha = if (tone == colors.onSurface) 0.1f else 0.16f), RoundedCornerShape(8.dp)).padding(horizontal = 12.dp, vertical = 6.dp), tone, style = MaterialTheme.typography.labelLarge)
                    }
                }
            }
            Preview(view)
            Text("What happens", Modifier.padding(start = 4.dp, top = 20.dp, bottom = 8.dp), style = MaterialTheme.typography.titleSmall)
            Row(horizontalArrangement = Arrangement.spacedBy(2.dp)) {
                Segment("Spam", view.does == RuleDoes.SPAM, RoundedCornerShape(topStart = 24.dp, bottomStart = 24.dp), Modifier.weight(1f)) { Store.send(Action.RuleSetDoes(RuleDoes.SPAM)) }
                Segment("Folder", view.does == RuleDoes.FOLDER, RoundedCornerShape(0.dp), Modifier.weight(1f)) { Store.send(Action.RuleSetDoes(RuleDoes.FOLDER)) }
                Segment("Label", view.does == RuleDoes.LABEL, RoundedCornerShape(topEnd = 24.dp, bottomEnd = 24.dp), Modifier.weight(1f)) { Store.send(Action.RuleSetDoes(RuleDoes.LABEL)) }
            }
            when (view.does) {
                RuleDoes.SPAM -> {}
                RuleDoes.FOLDER -> Target("Move to folder", view.folder, view.folders) { Store.send(Action.RuleSetFolder(it)) }
                RuleDoes.LABEL -> LabelTarget(view)
            }
            if (view.editing) {
                OutlinedButton(
                    { Store.send(Action.RuleDelete) },
                    Modifier.fillMaxWidth().padding(top = 12.dp).height(48.dp),
                    colors = ButtonDefaults.outlinedButtonColors(contentColor = colors.error),
                ) {
                    Glyph(Icons.Trash, null, colors.error, 20.dp)
                    Spacer(Modifier.width(8.dp))
                    Text("Delete rule", style = MaterialTheme.typography.labelLarge)
                }
            }
        }
    }
}

private fun lineOffset(text: String, chars: Int): Int = runCatching { text.offsetByCodePoints(0, chars) }.getOrDefault(text.length)

@Composable
private fun Preview(view: RuleEditorView) {
    val colors = MaterialTheme.colorScheme
    val tone = when (view.tone) {
        PreviewTone.GOOD -> Good
        PreviewTone.BAD -> colors.error
        PreviewTone.NEUTRAL -> colors.onSurfaceVariant
    }
    Column(Modifier.padding(top = 4.dp).fillMaxWidth().border(1.dp, colors.outlineVariant, RoundedCornerShape(12.dp))) {
        Row(Modifier.padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            when (view.tone) {
                PreviewTone.GOOD -> Glyph(Icons.Check, null, tone, 18.dp)
                PreviewTone.BAD -> Glyph(Icons.Spam, null, tone, 18.dp)
                PreviewTone.NEUTRAL -> {}
            }
            Text(view.message, style = MaterialTheme.typography.bodySmall, color = tone)
        }
        view.preview.forEach { row ->
            Box(Modifier.fillMaxWidth().height(1.dp).background(colors.outlineVariant))
            Row(Modifier.padding(horizontal = 16.dp, vertical = 10.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(row.who, Modifier.width(92.dp), style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(row.subject, style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        if (view.preview.isNotEmpty()) {
            Box(Modifier.fillMaxWidth().height(1.dp).background(colors.outlineVariant))
            Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(view.more.orEmpty(), Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
                TextButton({ Store.send(Action.RuleInSearch) }) { Text("Show all in search", color = Accent, style = MaterialTheme.typography.bodyMedium) }
            }
        }
    }
}

@Composable
private fun Segment(label: String, on: Boolean, shape: RoundedCornerShape, modifier: Modifier, onClick: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    Box(
        modifier.height(48.dp).background(if (on) colors.secondaryContainer else colors.surfaceContainer, shape).clickable(role = Role.RadioButton, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = if (on) colors.onSecondaryContainer else colors.onSurface, style = MaterialTheme.typography.labelLarge, fontWeight = if (on) FontWeight.Bold else FontWeight.Medium)
    }
}

@Composable
private fun Target(title: String, value: String, options: List<String>, pick: (String) -> Unit) {
    val colors = MaterialTheme.colorScheme
    var open by remember { mutableStateOf(false) }
    Box(Modifier.padding(top = 12.dp)) {
        Row(
            Modifier.fillMaxWidth().border(1.dp, colors.outlineVariant, RoundedCornerShape(12.dp)).clickable { open = true }.heightIn(min = 56.dp).padding(start = 16.dp, end = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(title, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
            Text(value, style = MaterialTheme.typography.bodyMedium, color = colors.onSurfaceVariant, maxLines = 1)
            Glyph(Icons.Down, null, size = 20.dp)
        }
        DropdownMenu(open, { open = false }) {
            options.forEach { option ->
                DropdownMenuItem({ Text(option) }, {
                    open = false
                    pick(option)
                })
            }
        }
    }
}

@Composable
private fun LabelTarget(view: RuleEditorView) {
    var typing by remember { mutableStateOf(view.labels.isEmpty()) }
    if (typing) {
        OutlinedTextField(
            view.label,
            { Store.send(Action.RuleSetLabel(it)) },
            Modifier.fillMaxWidth().padding(top = 12.dp),
            label = { Text("New label name") },
            singleLine = true,
        )
    } else {
        Target("Add label", view.label, view.labels + NEW_LABEL) {
            if (it == NEW_LABEL) {
                typing = true
                Store.send(Action.RuleSetLabel(""))
            } else {
                Store.send(Action.RuleSetLabel(it))
            }
        }
    }
}

private const val NEW_LABEL = "New label…"
