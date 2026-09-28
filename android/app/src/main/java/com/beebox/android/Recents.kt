package com.beebox.android

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject

/**
 * The links this phone has been in through, newest first.
 *
 * A link stays good until the owner revokes it: the daemon ties it to this
 * device by a secret the page keeps in its storage, so leaving a session and
 * coming back later is the same device returning, not a new one pairing.
 * Only a link that opened is kept, and one the daemon has disowned is dropped.
 */
class Recents(context: Context) {

    /** `name` is what the page said the link shows — a tab's title, say —
     *  once it had the tree; null until then. */
    data class Entry(val url: String, val name: String?)

    private val prefs = context.getSharedPreferences("recents", Context.MODE_PRIVATE)

    fun all(): List<Entry> {
        val json = prefs.getString(KEY, null) ?: return emptyList()
        return runCatching {
            val a = JSONArray(json)
            (0 until a.length()).map { a.getJSONObject(it) }.map {
                Entry(it.getString("url"), it.optString("name").ifEmpty { null })
            }
        }.getOrDefault(emptyList())
    }

    /** To the top, keeping the name it had unless a new one is known. */
    fun add(url: String, name: String?) {
        val rest = all()
        val entry = Entry(url, name ?: rest.find { it.url == url }?.name)
        save(listOf(entry) + rest.filter { it.url != url })
    }

    /** Renames in place: a tab's title changing is no reason to reorder. */
    fun rename(url: String, name: String) =
        save(all().map { if (it.url == url) it.copy(name = name) else it })

    fun remove(url: String) = save(all().filter { it.url != url })

    private fun save(list: List<Entry>) {
        val a = JSONArray()
        list.take(MAX).forEach { a.put(JSONObject().put("url", it.url).put("name", it.name ?: "")) }
        prefs.edit().putString(KEY, a.toString()).apply()
    }

    private companion object {
        const val KEY = "links"
        const val MAX = 5
    }
}
