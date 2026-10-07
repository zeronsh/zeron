package sh.zeron.android.ui

import android.content.res.Resources
import sh.zeron.android.R

/**
 * Elapsed-time label for the transcript Working row and the status pill
 * (iOS `StatusPill.elapsed`, extended past a day). The English form stays
 * pure Kotlin so the unit tests can run on the JVM; pass [Resources] for the
 * localized one (`1m 5s` / `1分5秒`).
 */
object ElapsedFormat {
    fun format(seconds: Long, res: Resources? = null): String {
        val s = seconds.coerceAtLeast(0)
        if (res == null) {
            return when {
                s < 60 -> "${s}s"
                s < 3600 -> "${s / 60}m ${s % 60}s"
                s < 86400 -> "${s / 3600}h ${s / 60 % 60}m"
                else -> "${s / 86400}d ${s / 3600 % 24}h"
            }
        }
        return when {
            s < 60 -> res.getString(R.string.elapsed_s, s.toInt())
            s < 3600 -> res.getString(R.string.elapsed_m_s, (s / 60).toInt(), (s % 60).toInt())
            s < 86400 -> res.getString(R.string.elapsed_h_m, (s / 3600).toInt(), (s / 60 % 60).toInt())
            else -> res.getString(R.string.elapsed_d_h, (s / 86400).toInt(), (s / 3600 % 24).toInt())
        }
    }
}
