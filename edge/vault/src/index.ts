/**
 * zeron-vault — the credential vault Worker (docs/design/cloud-device.md,
 * "Credential vault"). Holds users' provider credentials encrypted under a
 * KEK kept in Worker secrets (crypto.ts) and hands out short-lived grants
 * (access tokens, never refresh tokens) to enrolled, authorized devices.
 *
 * There is deliberately no HTTP surface: `fetch` is a constant 404 and the
 * Worker has no routes or workers.dev URL. The main edge Worker reaches it
 * through the `VAULT` service binding (`entrypoint: "VaultApi"`), passing the
 * identity it already verified; `VaultApi` implements `VaultRpc` (api.ts).
 */
import { WorkerEntrypoint } from "cloudflare:workers";
import type {
  ConnectionGrant,
  ConnectionGrantManyReply,
  ConnectionGrantManyRequest,
  ConnectionGrantRequest,
  EnrollDeviceRequest,
  GithubDevicePoll,
  GithubDeviceStart,
  GithubListingView,
  GithubRepoView,
  Grant,
  GrantManyReply,
  GrantManyRequest,
  GrantRequest,
  PutConnectionRequest,
  PutCredentialRequest,
  UpdateConnectionRequest,
  VaultAccountView,
  VaultCaller,
  VaultConnectionKind,
  VaultConnectionsView,
  VaultConnectionView,
  VaultProviderId,
  VaultResult,
  VaultRpc,
  VaultStatusView,
  VaultUsageView
} from "./api";
import { runCanary } from "./canary";
import type { Env } from "./env";
import { Vault } from "./vault";

export { VaultAccount } from "./account";
export { VaultConnections } from "./connections";
export { VaultDevices } from "./devices";

export class VaultApi extends WorkerEntrypoint<Env> implements VaultRpc {
  private vault(): Vault {
    return new Vault(this.env);
  }

  status(caller: VaultCaller): Promise<VaultResult<VaultStatusView>> {
    return this.vault().status(caller);
  }

  putCredential(
    caller: VaultCaller,
    provider: VaultProviderId,
    request: PutCredentialRequest
  ): Promise<VaultResult<VaultStatusView>> {
    return this.vault().putCredential(caller, provider, request);
  }

  authorize(
    caller: VaultCaller,
    provider: VaultProviderId,
    authorizedDevices: readonly string[]
  ): Promise<VaultResult<VaultStatusView>> {
    return this.vault().authorize(caller, provider, authorizedDevices);
  }

  disconnect(caller: VaultCaller, provider: VaultProviderId): Promise<VaultResult<VaultStatusView>> {
    return this.vault().disconnect(caller, provider);
  }

  enrollDevice(caller: VaultCaller, request: EnrollDeviceRequest): Promise<VaultResult<VaultStatusView>> {
    return this.vault().enrollDevice(caller, request);
  }

  revokeDevice(caller: VaultCaller, deviceId: string): Promise<VaultResult<VaultStatusView>> {
    return this.vault().revokeDevice(caller, deviceId);
  }

  setDisabled(caller: VaultCaller, disabled: boolean): Promise<VaultResult<VaultStatusView>> {
    return this.vault().setDisabled(caller, disabled);
  }

  grant(caller: VaultCaller, request: GrantRequest): Promise<VaultResult<Grant>> {
    return this.vault().grant(caller, request);
  }

  grantMany(caller: VaultCaller, request: GrantManyRequest): Promise<VaultResult<GrantManyReply>> {
    return this.vault().grantMany(caller, request);
  }

  githubDeviceStart(caller: VaultCaller, authorizedDevices: readonly string[]): Promise<VaultResult<GithubDeviceStart>> {
    return this.vault().githubDeviceStart(caller, authorizedDevices);
  }

  githubDevicePoll(caller: VaultCaller, flowId: string): Promise<VaultResult<GithubDevicePoll>> {
    return this.vault().githubDevicePoll(caller, flowId);
  }

  githubRepos(caller: VaultCaller, query?: string): Promise<VaultResult<readonly GithubRepoView[]>> {
    return this.vault().githubRepos(caller, query);
  }

  githubListing(caller: VaultCaller, query?: string): Promise<VaultResult<GithubListingView>> {
    return this.vault().githubListing(caller, query);
  }

  githubBranches(caller: VaultCaller, repo: string): Promise<VaultResult<readonly string[]>> {
    return this.vault().githubBranches(caller, repo);
  }

  accounts(caller: VaultCaller, provider: VaultProviderId): Promise<VaultResult<readonly VaultAccountView[]>> {
    return this.vault().accounts(caller, provider);
  }

  activateAccount(
    caller: VaultCaller,
    provider: VaultProviderId,
    slot: string | null
  ): Promise<VaultResult<readonly VaultAccountView[]>> {
    return this.vault().activateAccount(caller, provider, slot);
  }

  forgetAccount(caller: VaultCaller, provider: VaultProviderId, slot: string): Promise<VaultResult<readonly VaultAccountView[]>> {
    return this.vault().forgetAccount(caller, provider, slot);
  }

  accountUsage(caller: VaultCaller, provider: VaultProviderId, slot: string): Promise<VaultResult<VaultUsageView>> {
    return this.vault().accountUsage(caller, provider, slot);
  }

  listConnections(caller: VaultCaller, kind?: VaultConnectionKind): Promise<VaultResult<VaultConnectionsView>> {
    return this.vault().listConnections(caller, kind);
  }

  putConnection(
    caller: VaultCaller,
    id: string,
    request: PutConnectionRequest
  ): Promise<VaultResult<VaultConnectionView>> {
    return this.vault().putConnection(caller, id, request);
  }

  updateConnection(
    caller: VaultCaller,
    id: string,
    patch: UpdateConnectionRequest
  ): Promise<VaultResult<VaultConnectionView>> {
    return this.vault().updateConnection(caller, id, patch);
  }

  deleteConnection(caller: VaultCaller, id: string): Promise<VaultResult<{ readonly deleted: boolean }>> {
    return this.vault().deleteConnection(caller, id);
  }

  disconnectConnectionAuth(caller: VaultCaller, id: string): Promise<VaultResult<VaultConnectionView>> {
    return this.vault().disconnectConnectionAuth(caller, id);
  }

  grantConnection(caller: VaultCaller, request: ConnectionGrantRequest): Promise<VaultResult<ConnectionGrant>> {
    return this.vault().grantConnection(caller, request);
  }

  grantConnections(
    caller: VaultCaller,
    request: ConnectionGrantManyRequest
  ): Promise<VaultResult<ConnectionGrantManyReply>> {
    return this.vault().grantConnections(caller, request);
  }
}

export default {
  fetch: () => new Response("not found", { status: 404 }),
  scheduled: (_controller, env, ctx) => {
    ctx.waitUntil(runCanary(env));
  }
} satisfies ExportedHandler<Env>;
