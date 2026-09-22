package com.beebox.android

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AppCompatActivity
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.core.content.ContextCompat
import com.google.mlkit.vision.barcode.BarcodeScanning
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.common.InputImage
import java.util.concurrent.Executors

/**
 * The camera, pointed at a share code.
 *
 * Finishes the moment it reads one that looks like ours, handing the URL back
 * through the result Intent. Anything else on screen — a wifi code, a payment
 * code — is ignored rather than rejected loudly, so sweeping the camera across
 * a cluttered desk does not produce a stream of errors.
 */
class ScanActivity : AppCompatActivity() {

    private val worker = Executors.newSingleThreadExecutor()
    private val reader = BarcodeScanning.getClient()
    private var done = false

    private val askCamera = registerForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted -> if (granted) start() else finish() }

    override fun onCreate(saved: Bundle?) {
        super.onCreate(saved)
        setContentView(R.layout.scan)

        if (ContextCompat.checkSelfPermission(this, Manifest.permission.CAMERA)
            == PackageManager.PERMISSION_GRANTED
        ) {
            start()
        } else {
            askCamera.launch(Manifest.permission.CAMERA)
        }
    }

    override fun onDestroy() {
        super.onDestroy()
        worker.shutdown()
        reader.close()
    }

    private fun start() {
        val preview = findViewById<PreviewView>(R.id.preview)
        val future = ProcessCameraProvider.getInstance(this)
        future.addListener({
            val provider = future.get()

            val feed = Preview.Builder().build().also {
                it.surfaceProvider = preview.surfaceProvider
            }
            val analysis = ImageAnalysis.Builder()
                .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                .build()
                .also { it.setAnalyzer(worker, ::examine) }

            provider.unbindAll()
            provider.bindToLifecycle(
                this, CameraSelector.DEFAULT_BACK_CAMERA, feed, analysis,
            )
        }, ContextCompat.getMainExecutor(this))
    }

    @androidx.camera.core.ExperimentalGetImage
    private fun examine(proxy: ImageProxy) {
        val frame = proxy.image
        if (frame == null || done) {
            proxy.close()
            return
        }
        val image = InputImage.fromMediaImage(frame, proxy.imageInfo.rotationDegrees)
        reader.process(image)
            .addOnSuccessListener { codes -> codes.firstNotNullOfOrNull(::urlOf)?.let(::succeed) }
            .addOnCompleteListener { proxy.close() }
    }

    /** A code is ours if it addresses a BeeBox: our own scheme, or plain http
     *  to something that has a share path on it. */
    private fun urlOf(code: Barcode): String? {
        val raw = code.rawValue ?: return null
        val uri = runCatching { Uri.parse(raw) }.getOrNull() ?: return null
        return when {
            uri.scheme == "beebox" -> uri.buildUpon().scheme("http").build().toString()
            uri.scheme == "http" && uri.path?.startsWith("/t/") == true -> raw
            else -> null
        }
    }

    private fun succeed(url: String) {
        if (done) return
        done = true
        setResult(Activity.RESULT_OK, Intent().putExtra(EXTRA_URL, url))
        finish()
    }

    companion object {
        const val EXTRA_URL = "beebox.scanned"
    }
}
