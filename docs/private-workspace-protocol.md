# Private workspace reference

Private mode connects to the configured hub. It does not authenticate with WorkOS, contact the hosted Zeron sync service, or fall back to cloud sync. Agent providers and repository hosts retain their own network requirements.

Private data is stored under `profiles/private/<workspaceId>/`. Cloud and Local use separate profiles. The device-scoped `private.json` stores the hub address, node role, and credential with owner-only permissions on Unix. Tokens are excluded from status responses and debug output. The hub stores token hashes in SQLite.

The hub implements the existing registry, chat2, device relay, and blob protocols. SQLite commits registry operations and chat updates before acknowledgment. Active and open conversations stream updates. Other transcripts load on demand. Repository contents, agent credentials, and running processes stay on the owning agent server. Server shutdown does not transfer execution to another machine.

HTTP requests and WebSocket upgrades authenticate with `Authorization: Bearer`. Private tokens do not appear in connection URLs. Device-host relay connections and server-owned blob writes require a server credential bound to the matching device ID. Client nodes do not execute agent commands.

Pairing and access administration are available through local control on the hub. Relay clients cannot invoke those methods. Invitations carry a role, expire after five minutes, and are single-use. Failed attempts are limited. Re-pairing rotates the node credential and closes its previous connections. The hub cannot pair over its own identity.

Private mode supports conversations, queues, attachments, files, diffs, terminals, and agent controls through the existing protocols. Preview tunnels are unavailable. Desktop and mobile interfaces keep private connection credentials and caches separate from cloud state.

Tailscale Serve provides the private HTTPS endpoint. The hub binds only to loopback. Setup and hub startup check the selected Serve listener. Zeron does not configure Tailscale Funnel or reset unrelated Serve listeners.

Run the hub and runtime checks with:

```sh
scripts/test-private-workspace.sh
```

The tests use temporary stores, loopback listeners, and scripted agents. They do not configure the machine's Tailscale routes or send prompts to model providers.
