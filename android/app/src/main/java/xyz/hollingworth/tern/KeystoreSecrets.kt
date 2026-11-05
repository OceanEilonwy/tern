package xyz.hollingworth.tern

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Log
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import xyz.hollingworth.tern.core.SecretStore

/**
 * The session and the cache's key, encrypted with a key the Android Keystore holds and never hands
 * out. Without it there is nothing to read in these files, and they are left out of backups.
 */
class KeystoreSecrets(context: Context) : SecretStore {
    private val dir = context.noBackupFilesDir.resolve("secrets").apply { mkdirs() }

    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        val spec = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .build()
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
            init(spec)
            generateKey()
        }
    }

    private fun file(name: String): File = dir.resolve(name.toByteArray().joinToString("") { "%02x".format(it) })

    override fun get(name: String): ByteArray? = runCatching {
        val raw = file(name).takeIf { it.exists() }?.readBytes() ?: return null
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, raw, 0, IV_BYTES))
        cipher.doFinal(raw, IV_BYTES, raw.size - IV_BYTES)
    }.onFailure { Log.w("Tern", "could not read a secret: ${it.javaClass.simpleName}") }.getOrNull()

    override fun put(name: String, value: ByteArray) {
        runCatching {
            val cipher = Cipher.getInstance(TRANSFORM)
            cipher.init(Cipher.ENCRYPT_MODE, key())
            val target = file(name)
            val part = File(target.path + ".part")
            part.writeBytes(cipher.iv + cipher.doFinal(value))
            part.renameTo(target)
        }.onFailure { Log.w("Tern", "could not save a secret: ${it.javaClass.simpleName}") }
    }

    override fun remove(name: String) {
        file(name).delete()
    }

    private companion object {
        const val ALIAS = "tern-secrets"
        const val TRANSFORM = "AES/GCM/NoPadding"
        const val IV_BYTES = 12
    }
}
