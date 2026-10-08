package org.pastazzo.android

import android.content.Context
import android.graphics.Bitmap
import android.system.Os
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.ByteArrayOutputStream
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.security.KeyStore
import java.security.MessageDigest
import java.util.UUID
import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.RGBLuminanceSource
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import com.google.zxing.qrcode.QRCodeWriter

@RunWith(AndroidJUnit4::class)
class NativeTests {
    private val context = ApplicationProvider.getApplicationContext<Context>()
    private fun root() = File(context.noBackupFilesDir, "test-${UUID.randomUUID()}")

    @Test fun protectedKeysAreEncryptedNonexportableAndAuthenticated() {
        val secrets = AndroidSecrets(context)
        val user = "fixture-${UUID.randomUUID()}"
        val original = "synthetic protected key fixture with enough bytes".toByteArray()
        val input = original.copyOf()
        secrets.set(user, input)
        assertArrayEquals(ByteArray(input.size), input)
        assertArrayEquals(original, secrets.get(user))
        val id = MessageDigest.getInstance("SHA-256").digest(user.toByteArray()).joinToString("") { "%02x".format(it) }
        val file = File(context.noBackupFilesDir, "keys/$id.sealed")
        assertEquals(384, Os.stat(file.path).st_mode and 511)
        assertFalse(file.readBytes().toString(Charsets.ISO_8859_1).contains(original.toString(Charsets.UTF_8)))
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        assertNull(store.getKey("pastazzo.wrap.$id", null).encoded)
        val corrupted = file.readBytes().apply { this[lastIndex] = (this[lastIndex].toInt() xor 1).toByte() }
        file.writeBytes(corrupted)
        assertThrows(Exception::class.java) { secrets.get(user) }
        secrets.delete(user)
        assertFalse(file.exists())
    }

    @Test fun nativeHistoryPersistsTextImagesAndRejectsTraversal() = runBlocking {
        val root = root()
        try {
            val client = NativeClient(context, root)
            client.call("save", JSONObject().put("name", "Android QA").put("text", "A native Android fixture"))
            val bitmap = Bitmap.createBitmap(2, 2, Bitmap.Config.ARGB_8888)
            val bytes = ByteArrayOutputStream().apply { bitmap.compress(Bitmap.CompressFormat.PNG, 100, this) }.toByteArray()
            client.call("save", JSONObject().put("name", "Android QA").put("kind", "image").put("mime", "image/png").put("data", ClipboardAccess.encode(bytes)))
            val restarted = NativeClient(context, root)
            val items = restarted.call("history").getJSONArray("items")
            assertEquals(2, items.length())
            val image = (0 until items.length()).map { items.getJSONObject(it) }.first { it.getString("kind") == "image" }
            val data = restarted.call("item", JSONObject().put("id", image.getString("id"))).getJSONObject("item")
            assertArrayEquals(bytes, ClipboardAccess.decode(data.getString("data")))
            assertEquals(448, Os.stat(root.path).st_mode and 511)
            root.resolve("history").listFiles()!!.forEach { assertEquals(384, Os.stat(it.path).st_mode and 511) }
            try { restarted.call("item", JSONObject().put("id", "../../sync")); fail("Traversal accepted") } catch (_: IllegalStateException) {}
        } finally { root.deleteRecursively() }
    }

    private fun fixture(path: String, body: JSONObject? = null): JSONObject {
        val connection = URL("http://127.0.0.1:32952/$path").openConnection() as HttpURLConnection
        connection.connectTimeout = 5000; connection.readTimeout = 5000
        try {
            if (body != null) {
                connection.requestMethod = "POST"; connection.doOutput = true
                connection.setRequestProperty("Content-Type", "application/json")
                connection.outputStream.use { it.write(body.toString().toByteArray()) }
            }
            return JSONObject(connection.inputStream.bufferedReader().use { it.readText() })
        } finally { connection.disconnect() }
    }

    @Test fun nativeQrPairingSyncAndOfflineRetriesInteroperateWithDesktop() = runBlocking {
        val root = root()
        val client = NativeClient(context, root)
        try {
            val link = fixture("link").getString("link")
            val matrix = QRCodeWriter().encode(link, BarcodeFormat.QR_CODE, 1024, 1024)
            val pixels = IntArray(1024 * 1024) { i -> if (matrix[i % 1024, i / 1024]) android.graphics.Color.BLACK else android.graphics.Color.WHITE }
            val decoded = QRCodeReader().decode(BinaryBitmap(HybridBinarizer(RGBLuminanceSource(1024, 1024, pixels)))).text
            assertEquals(link, decoded)
            client.call("pair", JSONObject().put("link", decoded).put("name", "Android QA"))
            assertTrue(client.call("status").getBoolean("logged_in"))
            val metadata = root.resolve("sync.json").readText()
            assertFalse(metadata.contains("account_key")); assertFalse(metadata.contains("account_secret")); assertFalse(metadata.contains("synthetic fixture password"))
            assertTrue(metadata.contains("keychain"))
            assertFalse(root.resolve("approval-code").exists())
            client.call("save", JSONObject().put("name", "Android QA").put("text", "Hello from Android native"))
            fixture("send", JSONObject().put("text", "Hello from Mac Pro native"))
            client.call("refresh")
            val items = client.call("history").getJSONArray("items")
            assertTrue((0 until items.length()).map { items.getJSONObject(it) }.any { it.getString("origin") == "Mac Pro QA" && it.getString("preview") == "Hello from Mac Pro native" })
            fixture("offline", JSONObject().put("offline", true))
            val id = ClipboardAccess.encode(ByteArray(16).apply { java.security.SecureRandom().nextBytes(this) })
            val offlineText = "Android durable offline fixture ${UUID.randomUUID()}"
            val result = client.call("save", JSONObject().put("name", "Android QA").put("id", id).put("text", offlineText))
            assertTrue(result.getBoolean("queued"))
            val queued = root.resolve("mobile-outbox").walkTopDown().first { it.extension == "sealed" }
            val ciphertext = queued.readBytes()
            assertFalse(ciphertext.toString(Charsets.ISO_8859_1).contains(offlineText))
            fixture("offline", JSONObject().put("offline", false))
            val restarted = NativeClient(context, root)
            assertArrayEquals(ciphertext, queued.readBytes())
            restarted.call("refresh")
            assertFalse(queued.exists())
            assertFalse(restarted.call("item", JSONObject().put("id", id)).getJSONObject("item").getBoolean("queued"))
            val received = fixture("received").getJSONArray("items")
            assertEquals(1, (0 until received.length()).count { received.getJSONObject(it).optString("text") == offlineText })
            assertTrue((0 until received.length()).any { received.getJSONObject(it).optString("text") == "Hello from Android native" })
            restarted.call("logout")
            assertFalse(root.resolve("sync.json").exists())
        } finally {
            runCatching { fixture("offline", JSONObject().put("offline", false)) }
            if (root.resolve("sync.json").exists()) runCatching { client.call("logout") }
            root.deleteRecursively()
        }
    }
}
