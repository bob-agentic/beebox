package com.beebox.android

import android.content.Context
import android.content.Intent
import androidx.activity.result.contract.ActivityResultContract

/**
 * Reads one share code and returns the http URL it carried, or null.
 *
 * A contract rather than a bare Activity call so the caller does not have to
 * know about request codes or about how the camera is driven.
 */
class ScanContract : ActivityResultContract<Unit, String?>() {

    override fun createIntent(context: Context, input: Unit): Intent =
        Intent(context, ScanActivity::class.java)

    override fun parseResult(resultCode: Int, intent: Intent?): String? =
        intent?.getStringExtra(ScanActivity.EXTRA_URL)
}
