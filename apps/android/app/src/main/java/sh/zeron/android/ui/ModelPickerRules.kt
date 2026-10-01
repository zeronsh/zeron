package sh.zeron.android.ui

import sh.zeron.android.core.FavoriteModel
import uniffi.zeron_core.ModelInfo
import uniffi.zeron_core.ModelOption

/** One model a device offers, with the harness (provider) it runs under. */
data class ModelChoice(val harness: String, val harnessLabel: String, val model: ModelInfo) {
    val key get() = FavoriteModel(harness, model.id)
}

/** A provider whose model list is loading (error = null) or failed to load. */
data class CatalogStatus(val harness: String, val label: String, val error: String? = null)

/** The model settings the compact card lays out itself, or leaves to rows. */
sealed interface ModelSetting {
    data object Reasoning : ModelSetting
    data class Option(val id: String) : ModelSetting
}

/** Fast mode as a harness spells it: the option, the choice that turns it on and the one that turns it off. */
data class FastMode(val optionId: String, val on: String, val off: String)

/**
 * A model's option groups for the compact card (the desktop's `compact.rs` /
 * `pickers.rs`): fast mode is a specific option shape with its own button, the
 * rest become rows, and "Reset" returns to the model's defaults.
 */
object ModelOptions {
    /**
     * Fast mode's choices whatever form a harness gives it: a `fastMode` /
     * `fast_mode` toggle, or a tier/speed option offering `fast` beside its
     * default. An option that is already on by default offers no switch.
     */
    fun fastMode(option: ModelOption): FastMode? {
        fun has(id: String) = option.choices.any { it.id == id }
        if (option.id == "fastMode" || option.id == "fast_mode") {
            if (has("on")) {
                if (option.defaultChoice == "on") return null
                return FastMode(option.id, on = "on", off = option.defaultChoice)
            }
        }
        if (has("fast") && option.defaultChoice != "fast") return FastMode(option.id, on = "fast", off = option.defaultChoice)
        return null
    }

    /** The model's fast mode, from its first option shaped like one. */
    fun fastMode(model: ModelInfo?): FastMode? = model?.options?.firstNotNullOfOrNull { fastMode(it) }

    /** Whether fast mode is on given the explicit picks (unpicked = the option's default). */
    fun isFast(model: ModelInfo?, picks: Map<String, String>): Boolean {
        val fast = fastMode(model) ?: return false
        val option = model?.options?.firstOrNull { it.id == fast.optionId } ?: return false
        return (picks[fast.optionId] ?: option.defaultChoice) == fast.on
    }

    /** What tapping the fast button picks: (option id, choice id) — the opposite of now. */
    fun fastToggle(model: ModelInfo?, picks: Map<String, String>): Pair<String, String>? {
        val fast = fastMode(model) ?: return null
        return fast.optionId to if (isFast(model, picks)) fast.off else fast.on
    }

    /** Effort and fast mode have their own controls in the compact card; everything else is a row. */
    fun visible(setting: ModelSetting, fastId: String?): Boolean = when (setting) {
        ModelSetting.Reasoning -> false
        is ModelSetting.Option -> setting.id != fastId
    }

    /** The option groups the card lists as rows (nothing to choose between = no row). */
    fun rows(model: ModelInfo?): List<ModelOption> {
        val fastId = fastMode(model)?.optionId
        return model?.options.orEmpty().filter { it.choices.size > 1 && visible(ModelSetting.Option(it.id), fastId) }
    }

    /** The label of the option's current choice. */
    fun valueLabel(option: ModelOption, picks: Map<String, String>): String {
        val id = picks[option.id] ?: option.defaultChoice
        return option.choices.firstOrNull { it.id == id }?.label ?: option.choices.firstOrNull { it.id == option.defaultChoice }?.label.orEmpty()
    }

    /** The picks after choosing [choice] in [option]: choosing the default clears the pick. */
    fun with(picks: Map<String, String>, option: ModelOption, choice: String): Map<String, String> =
        if (choice == option.defaultChoice) picks - option.id else picks + (option.id to choice)

    /** The effort shown for [model]: the pick if the ladder has it, else the model's default. */
    fun effort(model: ModelInfo?, pick: String?): String? {
        val levels = model?.reasoningLevels.orEmpty()
        if (levels.isEmpty()) return null
        return pick?.takeIf { it in levels } ?: model?.defaultReasoning?.takeIf { it in levels } ?: levels[levels.size / 2]
    }

    /** Whether effort or any option is off its default (Reset has something to do). */
    fun differsFromDefaults(model: ModelInfo?, effortPick: String?, picks: Map<String, String>): Boolean {
        if (model == null) return false
        val defaultEffort = effort(model, null)
        val effortDiffers = defaultEffort != null && effort(model, effortPick) != defaultEffort
        val optionDiffers = model.options.any { o ->
            val pick = picks[o.id]
            pick != null && pick != o.defaultChoice && o.choices.any { it.id == pick }
        }
        return effortDiffers || optionDiffers
    }
}

/**
 * The model list's rules, after the desktop's `scoped_model_rows`: favorites
 * first, search over name and provider, and a current model the catalog
 * doesn't list kept as a "selected only" row.
 */
object ModelPickerRules {
    /** One row of the list. */
    data class Entry(val choice: ModelChoice, val starred: Boolean, val selectedOnly: Boolean = false) {
        val key get() = "${choice.harness}/${choice.model.id}"
    }

    /** Models visible in the list rows before the list scrolls; sizes the card. */
    const val VISIBLE_ROWS = 6

    /**
     * Prefix of the name ranks above a substring of it; the model id, the
     * provider and the description rank below those (the provider so "codex"
     * lists its models, the description so a provider attribution finds them).
     */
    fun rank(query: String, choice: ModelChoice): Int? {
        val q = query.trim().lowercase()
        if (q.isEmpty()) return 0
        fun of(text: String?, base: Int): Int? {
            val t = text?.lowercase() ?: return null
            return when {
                t.startsWith(q) -> base
                t.contains(q) -> base + 1
                else -> null
            }
        }
        return listOfNotNull(
            of(choice.model.label, 0),
            of(choice.model.id, 2),
            of(choice.harnessLabel, 4),
            of(choice.harness, 4),
            of(choice.model.description, 6),
        ).minOrNull()
    }

    /**
     * The list's rows. Without a query: starred models first, then the rest,
     * both in catalog order (so a provider's models stay together and the
     * order is the device's, not the order of starring). With one: matches only,
     * starred first, then best rank, then catalog order. A locked chat (an open
     * session can't change harness) lists only its own provider.
     */
    fun entries(
        catalog: List<ModelChoice>,
        favorites: List<FavoriteModel>,
        query: String,
        current: ModelChoice?,
        locked: Boolean,
    ): List<Entry> {
        val starred = favorites.toSet()
        val scope = if (locked && current != null) catalog.filter { it.harness == current.harness } else catalog
        val q = query.trim()
        val rows = if (q.isEmpty()) {
            scope.map { Entry(it, it.key in starred) }.sortedBy { !it.starred }
        } else {
            scope.mapIndexedNotNull { ix, c -> rank(q, c)?.let { r -> Triple(r, ix, Entry(c, c.key in starred)) } }
                .sortedWith(compareBy({ !it.third.starred }, { it.first }, { it.second }))
                .map { it.third }
        }
        // The selection is shown even when the catalog doesn't list it (a custom
        // model, or a list still loading) — never merged into the choices.
        if (current != null && rows.none { it.choice.key == current.key } && scope.none { it.key == current.key }) {
            val shown = q.isEmpty() || rank(q, current) != null
            if (shown) return listOf(Entry(current, current.key in starred, selectedOnly = true)) + rows
        }
        return rows
    }

    /**
     * Labels that more than one row carries (the same name from two providers,
     * or two variants): only those rows show their description, to tell them apart.
     */
    fun ambiguousLabels(entries: List<Entry>): Set<String> =
        entries.groupBy { it.choice.model.label.trim().lowercase() }.filterValues { it.size > 1 }.values
            .flatMap { rows -> rows.map { it.choice.model.label } }.toSet()

    /**
     * The first row to scroll to when the list opens on [selected]: the top if
     * the selection is among the first rows that fit, else it sits centred.
     */
    fun initialScrollIndex(selected: Int, visibleRows: Int = VISIBLE_ROWS): Int {
        if (selected < 0 || selected < visibleRows - 1) return 0
        return (selected - visibleRows / 2).coerceAtLeast(0)
    }

    /**
     * Where to scroll after a row changed place (a star re-sorts the list) so it
     * stays in view: nothing if it's still visible, else the nearest edge.
     */
    fun followScroll(newIndex: Int, firstVisible: Int, lastVisible: Int): Int? = when {
        newIndex < 0 -> null
        newIndex < firstVisible -> newIndex
        newIndex > lastVisible -> newIndex
        else -> null
    }
}

/** The composer chip's wording: "GPT-5.4  High", the effort dimmed. */
object ChipText {
    data class Parts(val model: String, val effort: String?, val fast: Boolean)

    fun parts(modelLabel: String, effort: String?, fast: Boolean, effortLabel: (String) -> String): Parts =
        Parts(modelLabel, effort?.takeIf { it.isNotEmpty() }?.let(effortLabel), fast)

    /** What a screen reader says for the chip. */
    fun describe(parts: Parts): String =
        listOfNotNull("Model: ${parts.model}", parts.effort?.let { "$it effort" }, if (parts.fast) "fast mode on" else null).joinToString(", ")
}
