package app.gipny

import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyInfo
import android.security.keystore.KeyProperties
import androidx.annotation.Keep
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.SecretKeyFactory
import javax.crypto.spec.GCMParameterSpec

/** Device binding only: the vault passphrase remains a separate requirement.
 * No raw AES wrapping key is exported to Rust, JS, profile files or backups.
 * Unwrap produces a short-lived binding secret; JNI must wipe its byte array.
 */
@Keep
object DeviceKeystore {
    private const val PROVIDER = "AndroidKeyStore"
    private const val PREFIX = "app.gipny.bind."

    private fun alias(id: String): String {
        require(id.matches(Regex("[a-f0-9]{64}"))) { "Invalid device binding identifier" }
        return PREFIX + id
    }

    private fun store(): KeyStore = KeyStore.getInstance(PROVIDER).apply { load(null) }

    private fun requireHardware(key: SecretKey) {
        val info = SecretKeyFactory.getInstance(key.algorithm, PROVIDER)
            .getKeySpec(key, KeyInfo::class.java) as KeyInfo
        val protected = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            info.securityLevel == KeyProperties.SECURITY_LEVEL_TRUSTED_ENVIRONMENT ||
                info.securityLevel == KeyProperties.SECURITY_LEVEL_STRONGBOX
        } else {
            legacyHardware(info)
        }
        check(protected) { "Hardware-backed Android Keystore is required" }
    }

    // The replacement API starts at Android 12; API 24–30 still use this
    // platform capability check. It never weakens to software-only storage.
    @Suppress("DEPRECATION")
    private fun legacyHardware(info: KeyInfo): Boolean = info.isInsideSecureHardware

    @JvmStatic
    @Synchronized
    fun seal(id: String, plaintext: ByteArray): ByteArray {
        try {
            return sealChecked(id, plaintext)
        } finally {
            // Identifier validation and provider initialization can fail before
            // a key exists. Their error paths also own this input array.
            plaintext.fill(0)
        }
    }

    private fun sealChecked(id: String, plaintext: ByteArray): ByteArray {
        val name = alias(id)
        val store = store()
        var created = false
        try {
            require(plaintext.size == 32) { "Invalid binding secret length" }
            val key = if (store.containsAlias(name)) {
                store.getKey(name, null) as? SecretKey ?: throw java.security.UnrecoverableKeyException("Device credential missing")
            } else {
                val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, PROVIDER)
                generator.init(KeyGenParameterSpec.Builder(name,
                    KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                    .setKeySize(256)
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .setRandomizedEncryptionRequired(true)
                    // The vault passphrase controls unlock. Do not invalidate
                    // long-term storage when biometric enrollment changes.
                    .setUserAuthenticationRequired(false)
                    .build())
                generator.generateKey().also { created = true }
            }
            requireHardware(key)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, key)
            cipher.updateAAD(id.toByteArray(Charsets.US_ASCII))
            val ciphertext = cipher.doFinal(plaintext)
            check(cipher.iv.size == 12 && ciphertext.size == 48) { "Invalid Keystore output" }
            return cipher.iv + ciphertext
        } catch (error: Exception) {
            // Only an alias created by this failed enrollment belongs to us.
            // Reads/capability failures never delete an existing credential.
            if (created) store.deleteEntry(name)
            throw error
        }
    }

    @JvmStatic
    @Synchronized
    fun unseal(id: String, wrapped: ByteArray): ByteArray {
        require(wrapped.size == 60) { "Invalid wrapped binding secret length" }
        val key = store().getKey(alias(id), null) as? SecretKey ?: throw java.security.UnrecoverableKeyException("Device credential missing")
        requireHardware(key)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, wrapped, 0, 12))
        cipher.updateAAD(id.toByteArray(Charsets.US_ASCII))
        val plaintext = cipher.doFinal(wrapped, 12, wrapped.size - 12)
        if (plaintext.size != 32) {
            plaintext.fill(0)
            error("Invalid unwrapped binding secret length")
        }
        return plaintext // Rust JNI copies into sealed memory and wipes this array.
    }

    @JvmStatic
    @Synchronized
    fun delete(id: String) { store().deleteEntry(alias(id)) }
}
