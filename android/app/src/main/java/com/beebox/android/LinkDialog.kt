package com.beebox.android

import android.app.Activity
import android.text.InputType
import android.widget.EditText
import android.widget.FrameLayout
import androidx.appcompat.app.AlertDialog

/** Typing the link out, for when there is no code to point a camera at. */
object LinkDialog {

    fun show(activity: Activity, onLink: (String) -> Unit) {
        val field = EditText(activity).apply {
            hint = activity.getString(R.string.link_hint)
            inputType = InputType.TYPE_TEXT_VARIATION_URI
            setSingleLine()
        }
        // AlertDialog gives an EditText no margins of its own.
        val pad = (activity.resources.displayMetrics.density * 20).toInt()
        val holder = FrameLayout(activity).apply {
            setPadding(pad, pad / 2, pad, 0)
            addView(field)
        }

        AlertDialog.Builder(activity)
            .setTitle(R.string.paste)
            .setView(holder)
            .setPositiveButton(R.string.connect) { _, _ ->
                field.text.toString().takeIf { it.isNotBlank() }?.let(onLink)
            }
            .setNegativeButton(android.R.string.cancel, null)
            .show()
    }
}
