package org.pastazzo.android

import android.app.Application
import android.content.Context
import androidx.test.runner.AndroidJUnitRunner
import java.io.File
import java.util.UUID

class TestApplication : Application() {
    private var testRoot: File? = null
    @Synchronized override fun getNoBackupFilesDir(): File {
        if (testRoot == null) testRoot = File(super.getNoBackupFilesDir(), "qa-${UUID.randomUUID()}").apply { mkdirs() }
        return testRoot!!
    }
}

class PastazzoTestRunner : AndroidJUnitRunner() {
    override fun newApplication(cl: ClassLoader?, className: String?, context: Context?): Application =
        super.newApplication(cl, TestApplication::class.java.name, context)
}
