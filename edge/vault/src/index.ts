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
  EnrollDeviceRequest,
  GithubDevicePoll,
  GithubDeviceStart,
  GithubRepoView,
  Grant,
  GrantRequest,
  PutCredentialRequest,
  VaultCaller,
  VaultProviderId,
  VaultResult,
  VaultRpc,
  VaultStatusView
} from "./api";
import { runCanary } from "./canary";
import type { Env } from "./env";
import { Vault } from "./vault";

export { VaultAccount } from "./account";
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

  githubDeviceStart(caller: VaultCaller, authorizedDevices: readonly string[]): Promise<VaultResult<GithubDeviceStart>> {
    return this.vault().githubDeviceStart(caller, authorizedDevices);
  }

  githubDevicePoll(caller: VaultCaller, flowId: string): Promise<VaultResult<GithubDevicePoll>> {
    return this.vault().githubDevicePoll(caller, flowId);
  }

  githubRepos(caller: VaultCaller, query?: string): Promise<VaultResult<readonly GithubRepoView[]>> {
    return this.vault().githubRepos(caller, query);
  }

  githubBranches(caller: VaultCaller, repo: string): Promise<VaultResult<readonly string[]>> {
    return this.vault().githubBranches(caller, repo);
  }
}

export default {
  fetch: () => new Response("not found", { status: 404 }),
  scheduled: (_controller, env, ctx) => {
    ctx.waitUntil(runCanary(env));
  }
} satisfies ExportedHandler<Env>;
