package org.pastazzo.android

import android.content.Context
import android.app.KeyguardManager
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.os.UserManager
import android.system.Os
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File
import java.security.KeyStore
import java.security.MessageDigest
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

object NativeBridge {
    init { System.loadLibrary("pastazzo_mobile") }
    @JvmStatic external fun call(input: ByteArray, secrets: AndroidSecrets): ByteArray
}

class AndroidSecrets(private val context: Context) {
    private val directory = File(context.noBackupFilesDir, "keys").apply { mkdirs(); Os.chmod(path, 448) }
    private val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    private fun id(user: String) = MessageDigest.getInstance("SHA-256")
        .digest(user.toByteArray()).joinToString("") { "%02x".format(it) }
    private fun alias(user: String) = "pastazzo.wrap.${id(user)}"
    private fun file(user: String) = File(directory, "${id(user)}.sealed")
    fun checkReady() {
        val keyguard = context.getSystemService(KeyguardManager::class.java)
        check(keyguard.isDeviceSecure) { "Set a screen lock in Android settings before connecting." }
        check(context.getSystemService(UserManager::class.java).isUserUnlocked && !keyguard.isDeviceLocked) { "Unlock this phone to access its keys." }
    }

    @Synchronized fun set(user: String, secret: ByteArray) {
        try {
            checkReady()
            val alias = alias(user)
            val key = if (store.containsAlias(alias)) store.getKey(alias, null) as SecretKey else {
                KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
                    init(KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                        .setKeySize(256).setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                        .setUnlockedDeviceRequired(true).build())
                }.generateKey()
            }
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, key)
            val temporary = File(directory, "${id(user)}.tmp")
            temporary.outputStream().use { output ->
                Os.chmod(temporary.path, 384)
                output.write(cipher.iv)
                output.write(cipher.doFinal(secret))
                output.fd.sync()
            }
            check(temporary.renameTo(file(user))) { "Could not save protected keys." }
        } finally { secret.fill(0) }
    }

    @Synchronized fun get(user: String): ByteArray {
        checkReady()
        val file = file(user)
        check(file.length() in 29..4096) { "Protected keys are unavailable." }
        val data = file.readBytes()
        val key = store.getKey(alias(user), null) as? SecretKey ?: error("Protected keys are unavailable.")
        return Cipher.getInstance("AES/GCM/NoPadding").run {
            init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, data.copyOfRange(0, 12)))
            doFinal(data, 12, data.size - 12)
        }
    }

    @Synchronized fun delete(user: String) {
        checkReady()
        val file = file(user)
        check(!file.exists() || file.delete()) { "Could not remove protected keys." }
        if (store.containsAlias(alias(user))) store.deleteEntry(alias(user))
    }
}

class NativeClient(context: Context, val root: File = File(context.noBackupFilesDir, "pastazzo")) {
    private val secrets = AndroidSecrets(context)
    private val mutex = Mutex()
    init { root.mkdirs(); Os.chmod(root.path, 448) }
    suspend fun call(operation: String, fields: JSONObject = JSONObject()): JSONObject = mutex.withLock {
        withContext(Dispatchers.IO) {
            if (operation == "pair" || operation == "login") secrets.checkReady()
            fields.put("operation", operation).put("root", root.path)
            val input = fields.toString().toByteArray()
            try {
                val output = NativeBridge.call(input, secrets)
                try {
                    val response = JSONObject(output.toString(Charsets.UTF_8))
                    check(response.getBoolean("ok")) { response.optString("error", "The operation failed.") }
                    response.getJSONObject("result")
                } finally { output.fill(0) }
            } finally { input.fill(0); fields.remove("password"); fields.remove("link") }
        }
    }
}
