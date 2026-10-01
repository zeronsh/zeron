package sh.zeron.android.core

import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import org.json.JSONArray
import org.json.JSONObject

/**
 * One starred model in the picker — harness + model id, like the desktop's
 * `FavoriteModel` (crates/ui/src/settings/composer.rs).
 */
data class FavoriteModel(val harness: String, val model: String)

/** The favorites list as stored: a JSON array of `{harness, model}`, in starring order. */
object FavoritesCodec {
    fun encode(list: List<FavoriteModel>): String =
        JSONArray().apply { list.forEach { put(JSONObject().put("harness", it.harness).put("model", it.model)) } }.toString()

    /** Corrupt or missing data reads as no favorites; duplicates and blanks are dropped. */
    fun decode(text: String?): List<FavoriteModel> {
        if (text.isNullOrBlank()) return emptyList()
        val array = runCatching { JSONArray(text) }.getOrNull() ?: return emptyList()
        return (0 until array.length()).mapNotNull { i ->
            val o = array.optJSONObject(i) ?: return@mapNotNull null
            val harness = o.optString("harness")
            val model = o.optString("model")
            if (harness.isEmpty() || model.isEmpty()) null else FavoriteModel(harness, model)
        }.distinct()
    }

    /** Star (appended, so the list keeps starring order) or unstar. */
    fun toggled(list: List<FavoriteModel>, favorite: FavoriteModel): List<FavoriteModel> =
        if (favorite in list) list - favorite else list + favorite
}

/**
 * Starred models, device-local (the desktop keeps them in its local
 * composer defaults too) and shared by every mode and device: a model
 * starred while driving one computer stays starred on the others.
 */
class FavoritesStore(private val prefs: SharedPreferences) {
    private val _favorites = MutableStateFlow(FavoritesCodec.decode(prefs.getString(KEY, null)))
    val favorites: StateFlow<List<FavoriteModel>> = _favorites.asStateFlow()

    fun toggle(favorite: FavoriteModel) {
        val next = FavoritesCodec.toggled(_favorites.value, favorite)
        _favorites.value = next
        prefs.edit().putString(KEY, FavoritesCodec.encode(next)).apply()
    }

    private companion object {
        const val KEY = "modelFavorites"
    }
}
