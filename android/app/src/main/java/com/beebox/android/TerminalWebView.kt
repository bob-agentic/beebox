package com.beebox.android

import android.content.Context
import android.text.InputType
import android.util.AttributeSet
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.webkit.WebView

/**
 * A WebView whose keyboard types commands, not prose.
 *
 * Asking for no suggestions is not enough: Samsung's keyboard went on
 * predicting with NO_SUGGESTIONS set. So the terminal says what Termux says —
 * TYPE_NULL, "this is not a text field" — and the keyboard drops prediction
 * altogether. Only the terminal: xterm's input is the one multi-line field on
 * the page, and the rename and path boxes keep their ordinary keyboard.
 */
class TerminalWebView @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : WebView(context, attrs) {
    override fun onCreateInputConnection(out: EditorInfo): InputConnection? {
        val ic = super.onCreateInputConnection(out)
        if (out.inputType and InputType.TYPE_TEXT_FLAG_MULTI_LINE != 0) {
            out.inputType = InputType.TYPE_NULL
        }
        return ic
    }
}
