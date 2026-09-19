# Run a private workspace through Tailscale

A private workspace uses a sync hub that you operate. Connect the hub, agent servers, and clients to the same tailnet before pairing them.

An **agent server** runs agents against its own repositories. A **client** controls those servers. The **sync hub** stores shared metadata and transcripts and relays connections. The hub can also be an agent server.

## Create the workspace

1. Install Zeron on the machine that will host the hub.
2. Connect Tailscale and enable MagicDNS and HTTPS for your tailnet.
3. Open **Settings → Workspace** and choose **Private via Tailscale**.
4. Create a workspace. Choose **Agent server** if this machine will also run agents.
5. Choose **Bring existing local work** if you want to copy local conversations into the private workspace. The original local profile remains available.
6. After setup, select **Run in background** to install the existing Zeron user service on Linux or macOS.

Finish active turns, close terminals, and save edited files before switching workspaces. A client that attaches to the background service can close without stopping the service.

The hub listens on `127.0.0.1:27655`. Setup adds a Tailscale Serve HTTPS listener on port `8443`. Local desktop control continues to use port `27654`. Setup refuses a conflicting route or Funnel configuration for the selected listener.

## Pair another device

1. On the hub, open **Settings → Workspace**.
2. Under **Add a device**, choose **Agent server** or **Client**, then select **Create invitation**.
3. On the other device, choose **Join private workspace**.
4. Enter the hub address and code, or open the invitation QR link.
5. Confirm the workspace and device role.

An invitation expires after five minutes and works once. Creating another invitation replaces the previous one. Five failed pairing attempts invalidate outstanding invitations. Keep pairing codes private until they are used or expire.

Paired devices can access the full workspace. Use Tailscale access rules to restrict which machines can reach the hub. Zeron also requires a separate credential for each paired device. Pairing does not copy agent credentials or repository files.

The hub's **Devices** list includes both clients and agent servers. A client without reported presence shows **Paired**. Use **Revoke** to remove another device's access. Expand **Connection details** for the hub address, access controls, and the option to leave the workspace.

## Use the command line

Create a hub:

```sh
zeron private create "Team workspace" --role server
zeron daemon install
```

Create an invitation on the hub:

```sh
zeron private pair --role server
```

Pair another agent server:

```sh
zeron private join https://hub.example.ts.net:8443 --code 123456 --name "Agent server" --role server
zeron daemon install
```

Replace the example address, code, and name with the values from your workspace. Use `--role client` when the device will only control agents.

`create` accepts `--listen-port` and `--serve-port` overrides. If a running engine receives a workspace change through the CLI, restart it after the command succeeds. The desktop setup flow performs this transition itself.

## Manage access

Inspect the workspace:

```sh
zeron private status
```

Revoke a node on the hub:

```sh
zeron private revoke DEVICE_ID
```

Revocation closes the node's connections immediately and rejects its credential. A fresh invitation can pair the device again.

Disable or restore private access:

```sh
zeron private disable
zeron private enable
```

On the hub, disabling access closes remote connections and removes its Tailscale Serve listener. On another node, it disconnects that node from the workspace. Local work and saved workspace data remain on disk.

Leave the workspace:

```sh
zeron private leave
```

Leaving removes this machine's private connection credentials. Saved conversations remain in the private profile. The next startup uses Local mode. Use **Zeron Cloud** in workspace settings to configure hosted sync separately.
