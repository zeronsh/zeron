package sh.zeron.android.core

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import sh.zeron.android.ZeronApplication

/** Accept / Decline on an incoming-transfer notification. */
class TransferActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val id = intent.getStringExtra(EXTRA_ID) ?: return
        val model = (context.applicationContext as ZeronApplication).model
        model.ensureBooted()
        model.notifier.cancelTransfer(id)
        val pending = goAsync()
        model.transfers.respond(id, accept = intent.action == ACCEPT) { pending.finish() }
    }

    companion object {
        const val ACCEPT = "sh.zeron.android.transfer.ACCEPT"
        const val DECLINE = "sh.zeron.android.transfer.DECLINE"
        const val EXTRA_ID = "transferId"
    }
}
