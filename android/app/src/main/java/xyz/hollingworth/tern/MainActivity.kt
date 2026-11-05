package xyz.hollingworth.tern

import android.content.ActivityNotFoundException
import android.content.Intent
import android.graphics.Color
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.FileProvider
import androidx.lifecycle.lifecycleScope
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import com.tern.engine.Action
import com.tern.engine.Effect
import xyz.hollingworth.tern.ui.Root

/** Hosts the screen and does what only the platform can: intents, and the file pickers. */
class MainActivity : ComponentActivity() {
    private var toSave: File? = null

    private val saver = registerForActivityResult(ActivityResultContracts.CreateDocument("*/*")) { target ->
        val source = toSave
        if (target != null && source != null) {
            lifecycleScope.launch(Dispatchers.IO) {
                contentResolver.openOutputStream(target)?.use { out -> source.inputStream().use { it.copyTo(out) } }
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        // Tern is always dark: the system bars' icons are light whatever the phone's theme is.
        enableEdgeToEdge(SystemBarStyle.dark(Color.TRANSPARENT), SystemBarStyle.dark(Color.TRANSPARENT))
        super.onCreate(savedInstanceState)
        setContent { Root() }
        lifecycleScope.launch { Store.effects.collect { perform(it) } }
        if (savedInstanceState == null) takeLink(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        takeLink(intent)
    }

    override fun onStart() {
        super.onStart()
        Store.send(Action.Foreground)
    }

    override fun onStop() {
        super.onStop()
        Store.send(Action.Hidden)
    }

    private fun takeLink(intent: Intent?) {
        val key = intent?.getStringExtra(EXTRA_KEY)
        val folder = intent?.getStringExtra(EXTRA_FOLDER)
        if (key != null && folder != null) {
            intent.removeExtra(EXTRA_KEY)
            Store.send(Action.OpenThread(key, folder))
            return
        }
        val link = intent?.data?.takeIf { it.scheme.equals("mailto", ignoreCase = true) } ?: return
        Store.send(Action.OpenLink(link.toString()))
    }

    private suspend fun perform(effect: Effect) {
        when (effect) {
            is Effect.OpenUrl -> launch(Intent(Intent.ACTION_VIEW, Uri.parse(effect.url)))
            is Effect.OpenFile -> {
                val uri = withContext(Dispatchers.IO) { FileProvider.getUriForFile(this@MainActivity, "$packageName.files", File(effect.path)) }
                launch(Intent(Intent.ACTION_VIEW).setDataAndType(uri, effect.mime).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION))
            }
            is Effect.SaveFile -> {
                toSave = File(effect.path)
                saver.launch(effect.name)
            }
            Effect.ClearNotifications -> clearNotices(this)
            is Effect.Log -> {}
        }
    }

    private fun launch(intent: Intent) {
        try {
            startActivity(intent)
        } catch (_: ActivityNotFoundException) {
            // Nothing on this phone opens it; the mail stays where it is.
        }
    }
}
