import type { PreviewService } from "@zeron/proto";

/**
 * Preview derivations for the web client — the browser-side peer of the
 * desktop browser surface's preview list (crates/ui/src/browser/view.rs)
 * over the engine's `WatchPreviews` stream (crates/proto/src/preview.rs).
 * Pure; unit-tested in tests/preview.test.ts.
 *
 * Mechanism (ticket 15): a discovered dev server is framed through the
 * engine's preview proxy. The engine reverse-proxies
 * `{device}.{service}.localhost:{proxyPort}` on its own loopback, routing
 * by Host header (crates/preview/src/proxy.rs), so an iframe at that
 * origin renders the dev server — WebSocket upgrades included — with no
 * new protocol work. The URL resolves only on the engine's own machine,
 * so framing is gated on the browser having reached the engine over
 * loopback (the web peer of the desktop's `EngineKey::is_local` gate in
 * browser/mod.rs watch_previews).
 *
 * Preview identities and names are engine-owned and persisted
 * (crates/preview/src/catalog.rs): names survive dev-server restarts and
 * engine restarts unchanged. The web client surfaces them as emitted —
 * rows are keyed by `service.id`, never renamed or re-derived here.
 *
 * The off-machine path (a phone browser reaching the engine over the LAN)
 * is DEFERRED, as ticket 15 allows: `*.localhost` resolves to the
 * browser's own loopback there and the proxy binds loopback only, so
 * framing needs browser↔engine WebRTC peering — a new authenticated
 * signaling RPC surface (the engine's own peer signaling is not wired to
 * a coordinator yet: the `Peers` output channel is dropped in
 * crates/preview/src/service.rs), a browser reimplementation of the
 * preview mux framing (crates/preview/src/mux.rs), and a ServiceWorker
 * fetch bridge, since an iframe cannot load HTTP over a DataChannel.
 * Until then the panel lists discovered services off-machine with an
 * honest note. Workaround: SSH port-forward the engine port AND the
 * proxy port (7331) — the engine then classifies as loopback and frames.
 */

/** The framed URL of a service — port of Rust `PreviewService::url`. */
export function previewUrl(service: Pick<PreviewService, "hostname">, proxyPort: number): string {
  return `http://${service.hostname}:${proxyPort}`;
}

/** Row sub-line, mirroring the desktop: remote rows name their device. */
export function previewRowLabel(service: Pick<PreviewService, "deviceName" | "port">, remote: boolean): string {
  return remote ? `${service.deviceName} · localhost:${service.port}` : `localhost:${service.port}`;
}

/** List subtitle, mirroring the desktop's remote/local wording. */
export function previewSubtitle(remote: boolean): string {
  return remote ? "Running on your device" : "Running locally";
}

/** The empty-list message, mirroring the desktop's three states. */
export function previewEmptyCopy(loading: boolean, remote: boolean): string {
  if (loading) {
    return "Looking for dev servers…";
  }
  return remote
    ? "Start a dev server in this project on your other device. Its preview will appear here when that device is online."
    : "Start a dev server in this project. It will appear here automatically, ready to open.";
}

/** Loopback names a browser can only resolve on the engine's own machine. */
export function isLoopbackHost(hostname: string): boolean {
  let host = hostname.toLowerCase();
  if (host.startsWith("[") && host.endsWith("]")) {
    host = host.slice(1, -1);
  }
  if (host === "localhost" || host.endsWith(".localhost") || host === "::1") {
    return true;
  }
  return /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host);
}

/**
 * Whether this browser can reach the preview proxy of the engine at
 * `baseUrl`. The proxy listens on the engine's loopback only, so reach is
 * exactly "the engine's endpoint is loopback from here" — a browser that
 * dialed the engine over loopback shares its loopback namespace (this
 * includes SSH port-forwards, which is why the check keys on the endpoint
 * rather than anything engine-reported).
 */
export function previewProxyReachable(baseUrl: string): boolean {
  try {
    return isLoopbackHost(new URL(baseUrl).hostname);
  } catch {
    return false;
  }
}
