package app.gipny

import androidx.test.ext.junit.runners.AndroidJUnit4
import java.security.UnrecoverableKeyException
import java.util.UUID
import javax.crypto.AEADBadTagException
import org.junit.Assert.*
import org.junit.Assume.assumeNoException
import org.junit.Test
import org.junit.runner.RunWith

/** Native platform tests; hardware-positive cases require a real TEE/StrongBox.
 * A software-only emulator must refuse enrollment, never pass as hardware.
 */
@RunWith(AndroidJUnit4::class)
class DeviceKeystoreTest {
    private fun id(): String = UUID.randomUUID().toString().replace("-", "") +
        UUID.randomUUID().toString().replace("-", "")

    @Test fun missingAliasIsNeverRecreated() {
        val id = id()
        try {
            DeviceKeystore.unseal(id, ByteArray(60))
            fail("A missing OS credential must fail")
        } catch (_: UnrecoverableKeyException) { /* expected */ }
        DeviceKeystore.delete(id)
    }

    @Test fun hardwareRoundTripAndAuthenticatedTamper() {
        val id = id()
        val plaintext = ByteArray(32) { (it + 1).toByte() }
        var output: ByteArray? = null
        try {
            val wrapped = try {
                DeviceKeystore.seal(id, plaintext)
            } catch (error: IllegalStateException) {
                assertArrayEquals("Failed seal must wipe its input", ByteArray(32), plaintext)
                assertEquals("Hardware-backed Android Keystore is required", error.message)
                // Report a skipped hardware-positive test on software-only
                // systems, rather than claiming a successful protected roundtrip.
                assumeNoException("TEE/StrongBox is required for this test", error)
                return
            }
            assertArrayEquals(ByteArray(32), plaintext)
            assertEquals(60, wrapped.size)
            output = DeviceKeystore.unseal(id, wrapped)
            assertArrayEquals(ByteArray(32) { (it + 1).toByte() }, output)
            wrapped[wrapped.lastIndex] = (wrapped.last().toInt() xor 1).toByte()
            try {
                DeviceKeystore.unseal(id, wrapped).fill(0)
                fail("Modified ciphertext must not decrypt")
            } catch (_: AEADBadTagException) { /* expected */ }
        } finally {
            plaintext.fill(0)
            output?.fill(0)
            DeviceKeystore.delete(id)
        }
    }

    @Test fun invalidIdentifierWipesSealInput() {
        val secret = ByteArray(32) { 1 }
        try {
            DeviceKeystore.seal("not-an-identifier", secret)
            fail("Malformed identifier must fail")
        } catch (_: IllegalArgumentException) { /* expected */ }
        assertArrayEquals(ByteArray(32), secret)
    }
}
