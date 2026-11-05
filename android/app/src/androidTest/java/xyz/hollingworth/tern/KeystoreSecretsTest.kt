package xyz.hollingworth.tern

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Test
import org.junit.runner.RunWith
import xyz.hollingworth.tern.core.SecretException

/** The one piece of logic only the platform can run: secrets through the real Android Keystore. */
@RunWith(AndroidJUnit4::class)
class KeystoreSecretsTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext

    @Test
    fun secretsComeBackAcrossInstancesAndAreEncryptedOnDisk() {
        val value = "the secret session".toByteArray()
        val first = KeystoreSecrets(context)
        first.put("test-secret", value)
        // A new instance is what the next start of the app sees.
        assertArrayEquals(value, KeystoreSecrets(context).get("test-secret"))
        val files = context.noBackupFilesDir.resolve("secrets").listFiles().orEmpty()
        assertFalse(files.any { String(it.readBytes(), Charsets.ISO_8859_1).contains("the secret session") })
        first.put("test-secret", "changed".toByteArray())
        assertArrayEquals("changed".toByteArray(), first.get("test-secret"))
        first.remove("test-secret")
        assertNull(first.get("test-secret"))
        assertNull(first.get("never-saved"))
    }

    @Test
    fun aSecretThatCannotBeReadIsAnErrorAndNotAbsent() {
        val secrets = KeystoreSecrets(context)
        secrets.put("damaged-secret", "the key to the mail".toByteArray())
        val dir = context.noBackupFilesDir.resolve("secrets")
        val name = "damaged-secret".toByteArray().joinToString("") { "%02x".format(it) }
        dir.resolve(name).writeBytes(ByteArray(40))
        try {
            assertThrows(SecretException.Unavailable::class.java) { secrets.get("damaged-secret") }
        } finally {
            secrets.remove("damaged-secret")
        }
    }
}
