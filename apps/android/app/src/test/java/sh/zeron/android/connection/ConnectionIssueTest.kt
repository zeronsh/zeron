package sh.zeron.android.connection

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import sh.zeron.android.core.ConnectionIssue
import sh.zeron.android.core.ConnectionIssue.Kind

/** Messages as crates/client/src/direct words them (ssh.rs, host.rs, mod.rs). */
class ConnectionIssueTest {
    private fun k(s: String?) = ConnectionIssue.classify(s)

    @Test fun network() {
        assertEquals(Kind.TIMEOUT, k("timed out reaching 192.168.1.20:22"))
        assertEquals(Kind.REFUSED, k("192.168.1.20:22 refused the connection — is OpenSSH Server running?"))
        assertEquals(Kind.UNREACHABLE, k("can't reach 192.168.1.20:22 (IO error: No route to host (os error 113))"))
        assertEquals(Kind.UNREACHABLE, k("can't reach 10.0.0.9:22 (IO error: Network is unreachable (os error 101))"))
        assertEquals(Kind.DNS, k("can't reach studio.local:22 (IO error: failed to lookup address information: No address associated with hostname)"))
    }

    @Test fun credentials() {
        assertEquals(Kind.AUTH_KEY, k("the machine rejected this phone's key for user \"dev\" — add the public key to authorized_keys (administrators_authorized_keys for admin accounts on Windows)"))
        assertEquals(Kind.AUTH_PASSWORD, k("wrong password for user \"dev\""))
        assertEquals(Kind.KEY, k("can't read the private key: invalid format"))
        assertEquals(Kind.HOST_KEY_UNKNOWN, k("unknown host key ssh-ed25519 SHA256:abc"))
        assertEquals(Kind.HOST_KEY_CHANGED, k("HOST KEY CHANGED: expected SHA256:abc, got ssh-ed25519 SHA256:def"))
        assertTrue(Kind.HOST_KEY_CHANGED.needsUser)
        assertFalse(Kind.TIMEOUT.needsUser)
    }

    @Test fun engine() {
        assertEquals(Kind.ENGINE, k("the machine refused a tunnel to 127.0.0.1:27654 (Channel open failure). Is Zeron running there?"))
        assertEquals(Kind.ENGINE, k("no Zeron engine answered on 127.0.0.1:27654 (connection closed)"))
        assertEquals(Kind.ENGINE, k("EngineInfo timed out"))
        assertEquals(Kind.ENGINE, k("timed out opening the tunnel"))
    }

    @Test fun linkLoss() {
        assertEquals(Kind.LOST, k("connection to the machine lost"))
        assertEquals(Kind.LOST, k("the SSH session closed"))
        assertEquals(Kind.SYNC, k("couldn't read WatchChats from the engine: bad frame"))
    }

    @Test fun unknownAndEmpty() {
        assertEquals(Kind.UNKNOWN, k(null))
        assertEquals(Kind.UNKNOWN, k("  "))
        assertEquals(Kind.UNKNOWN, k("something odd"))
    }
}
