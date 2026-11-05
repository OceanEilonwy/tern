package xyz.hollingworth.tern.ui

import androidx.activity.compose.PredictiveBackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.ContentTransform
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import kotlin.coroutines.cancellation.CancellationException
import kotlinx.coroutines.delay
import xyz.hollingworth.tern.Store
import xyz.hollingworth.tern.core.Action
import xyz.hollingworth.tern.core.Screen

private const val SLIDE_MS = 320
private const val FADE_MS = 160

/** What fills the window. On a wide screen the list and the conversation are one place. */
private enum class Place { Starting, SignIn, Mail, Conversation, Composer, Search, Settings, Rules, RuleEditor }

private fun Screen.place(wide: Boolean) = when (this) {
    Screen.STARTING -> Place.Starting
    Screen.SIGN_IN -> Place.SignIn
    Screen.INBOX -> Place.Mail
    Screen.CONVERSATION -> if (wide) Place.Mail else Place.Conversation
    Screen.COMPOSER -> Place.Composer
    Screen.SEARCH -> Place.Search
    Screen.SETTINGS -> Place.Settings
    Screen.RULES -> Place.Rules
    Screen.RULE_EDITOR -> Place.RuleEditor
}

@Composable
fun Root() {
    val shell by Store.shell.collectAsState()
    val sheet by Store.sheet.collectAsState()
    val list by Store.list.collectAsState()
    val composer by Store.composer.collectAsState()
    val stack = shell?.stack ?: listOf(Screen.STARTING)
    TernTheme(shell?.pureBlack ?: true, shell?.matchWallpaper ?: false) {
        BoxWithConstraints(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
            val wide = maxWidth >= 600.dp
            val place = stack.last().place(wide)
            // Which way the last change went decides which way the screens slide.
            var depth by remember { mutableIntStateOf(stack.size) }
            val forward = stack.size >= depth
            LaunchedEffect(stack.size) { depth = stack.size }

            // Back through the engine, with the screen shrinking under the gesture as Android 17 expects.
            val selecting = list?.selection != null
            val canGoBack = stack.size > 1 || sheet != null || selecting
            var backProgress by remember { mutableFloatStateOf(0f) }
            PredictiveBackHandler(enabled = canGoBack) { progress ->
                try {
                    progress.collect { if (sheet == null && !selecting && !place.isFirst) backProgress = it.progress }
                    backProgress = 0f
                    val writing = stack.last() == Screen.COMPOSER && sheet == null
                    Store.send(if (writing) Action.ComposeClose(ComposerText.current(composer)) else Action.Back)
                } catch (e: CancellationException) {
                    backProgress = 0f
                    throw e
                }
            }
            val shrink by animateFloatAsState(backProgress, tween(80), label = "back")

            val slide = with(LocalDensity.current) { 48.dp.roundToPx() }
            AnimatedContent(
                targetState = place,
                modifier = Modifier.fillMaxSize().graphicsLayer {
                    val scale = 1f - shrink * 0.1f
                    scaleX = scale
                    scaleY = scale
                    translationX = shrink * size.width * 0.06f
                    clip = shrink > 0f
                    shape = RoundedCornerShape((shrink * 28).dp)
                },
                transitionSpec = {
                    val composing = targetState == Place.Composer || initialState == Place.Composer
                    val first = initialState == Place.Starting || targetState == Place.Starting
                    when {
                        first -> fadeIn(tween(SLIDE_MS)) togetherWith fadeOut(tween(FADE_MS))
                        composing && targetState == Place.Composer -> ContentTransform(
                            slideInVertically(tween(SLIDE_MS, easing = Emphasized)) { it / 6 } + fadeIn(tween(FADE_MS)),
                            fadeOut(tween(FADE_MS)),
                            targetContentZIndex = 1f,
                        )
                        composing -> ContentTransform(
                            fadeIn(tween(FADE_MS)),
                            slideOutVertically(tween(SLIDE_MS, easing = Emphasized)) { it / 6 } + fadeOut(tween(FADE_MS)),
                            targetContentZIndex = -1f,
                        )
                        forward -> slideInHorizontally(tween(SLIDE_MS, easing = Emphasized)) { slide } + fadeIn(tween(SLIDE_MS / 2, SLIDE_MS / 4)) togetherWith
                            slideOutHorizontally(tween(SLIDE_MS, easing = Emphasized)) { -slide } + fadeOut(tween(FADE_MS))
                        else -> slideInHorizontally(tween(SLIDE_MS, easing = Emphasized)) { -slide } + fadeIn(tween(SLIDE_MS / 2, SLIDE_MS / 4)) togetherWith
                            slideOutHorizontally(tween(SLIDE_MS, easing = Emphasized)) { slide } + fadeOut(tween(FADE_MS))
                    }
                },
                label = "screens",
            ) { showing ->
                when (showing) {
                    Place.Starting -> Box(Modifier.fillMaxSize())
                    Place.SignIn -> SignInScreen()
                    Place.Mail -> MailScreen(wide, stack.last() == Screen.CONVERSATION)
                    Place.Conversation -> ConversationScreen(pane = false)
                    Place.Composer -> ComposerScreen()
                    Place.Search -> SearchScreen()
                    Place.Settings -> SettingsScreen()
                    Place.Rules -> RulesScreen()
                    Place.RuleEditor -> RuleEditorScreen()
                }
            }
            Snack(Modifier.align(Alignment.BottomCenter))
            Sheets()
        }
    }
}

private val Place.isFirst get() = this == Place.Mail || this == Place.SignIn || this == Place.Starting

/** The header's second line, and the one thing on every screen that says how the connection is. */
@Composable
fun Snack(modifier: Modifier = Modifier) {
    val shell by Store.shell.collectAsState()
    val snack = shell?.snack
    var shown by remember { mutableStateOf(snack) }
    if (snack != null) shown = snack
    LaunchedEffect(snack?.id) {
        val id = snack?.id ?: return@LaunchedEffect
        delay(if (snack.action != null) 8000 else 4000)
        Store.send(Action.SnackGone(id))
    }
    AnimatedVisibility(
        visible = snack != null,
        modifier = modifier.navigationBarsPadding().padding(start = 16.dp, end = 16.dp, bottom = 100.dp).widthIn(max = 560.dp).fillMaxWidth(),
        enter = slideInVertically(tween(240, easing = Emphasized)) { it / 2 } + fadeIn(tween(120)),
        exit = fadeOut(tween(120)) + scaleOut(tween(120), targetScale = 0.96f),
    ) {
        val current = shown ?: return@AnimatedVisibility
        val colors = MaterialTheme.colorScheme
        Row(
            Modifier.fillMaxWidth().background(colors.inverseSurface, RoundedCornerShape(4.dp)).padding(start = 16.dp, end = 8.dp).height(48.dp).semantics { liveRegion = LiveRegionMode.Polite },
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(current.text, Modifier.weight(1f), colors.inverseOnSurface, style = MaterialTheme.typography.bodyMedium, maxLines = 2)
            current.action?.let { action ->
                TextButton(onClick = { Store.send(Action.SnackAction) }) { Text(action, color = colors.inversePrimary, style = MaterialTheme.typography.labelLarge) }
            }
        }
    }
}
