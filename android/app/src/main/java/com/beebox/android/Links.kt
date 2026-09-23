package com.beebox.android

import android.net.Uri

/**
 * What counts as a link to a BeeBox, decided in one place.
 *
 * The scanner, the system camera's intent and the typed link each used to
 * carry their own rule, and they disagreed: the scanner only knew the Tab
 * scope, so a Workspace code was ignored by the in-app scanner while the
 * system camera opened it fine.
 */
object Links {

    /** The share scopes the daemon routes: All, Workspace, Tab, Pane. */
    private val SCOPES = setOf("a", "w", "t", "p")

    /** A code read off a screen. Strict, because the camera sees every code
     *  on the desk: only our own scheme, or a web link with a share path. */
    fun fromCode(raw: String): String? {
        val uri = parse(raw) ?: return null
        val shared = uri.pathSegments.size == 2 && uri.pathSegments[0] in SCOPES
        return when {
            uri.scheme == "beebox" -> toWeb(uri)
            uri.scheme in WEB && shared -> uri.toString()
            else -> null
        }
    }

    /** A link someone typed or pasted. Lenient about form — no scheme, stray
     *  whitespace — but still has to name a host. Any path is allowed: the
     *  owner's own `?key=` address is a legitimate thing to paste. */
    fun fromTyped(raw: String): String? {
        val text = raw.trim()
        // Without a scheme Uri.parse reads the host as one:
        // `192.168.1.10:17788/t/x` comes back with scheme `192.168.1.10`.
        val uri = parse(if ("://" in text) text else "http://$text") ?: return null
        return when (uri.scheme) {
            "beebox" -> toWeb(uri)
            in WEB -> uri.toString()
            else -> null
        }
    }

    /** The scheme exists only to be claimable by the camera; the daemon
     *  speaks http. Nothing else about the link is rewritten. */
    fun toWeb(uri: Uri): String = uri.buildUpon().scheme("http").build().toString()

    /** Scheme, host and port — with the default port filled in, since a URL
     *  that omits it and one that spells it out address the same daemon. */
    fun origin(uri: Uri): String = "${uri.scheme}://${uri.host}:${port(uri)}"

    private fun port(uri: Uri): Int = when {
        uri.port != -1 -> uri.port
        uri.scheme == "https" -> 443
        else -> 80
    }

    private fun parse(raw: String): Uri? =
        runCatching { Uri.parse(raw.trim()) }.getOrNull()?.takeIf { !it.host.isNullOrEmpty() }

    private val WEB = setOf("http", "https")
}
