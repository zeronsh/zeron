/**
 * The vault's authorization layer: every `VaultRpc` method, as plain code over
 * an `Env`, so tests can drive it with any env (kill switches) and VaultApi
 * stays a thin WorkerEntrypoint shell.
 *
 * Order of checks for every call:
 * 1. Global kill switch (`VAULT_DISABLED=1`) → `disabled` 503, before anything
 *    else is even parsed.
 * 2. Caller shape; `runner` callers may ONLY call `grant`, and only for their
 *    own device (the runner JWT's `dev` claim).
 * 3. Per-user kill switch → `disabled` 403 for everything that reads the vault
 *    or adds access. Calls that only REMOVE access (revoke, disconnect) and the
 *    switch itself stay available, so a user who pulled the switch can still
 *    clean up and switch it back.
 * 4. Grants additionally need: device enrolled + not revoked (nor its parent),
 *    a valid fresh signature (VaultDevices), the device OR its parent ∈
 *    authorizedDevices, status connected (VaultAccount).
 */
import {
  VAULT_PROVIDERS,
  type EnrollDeviceRequest,
  type GithubDevicePoll,
  type GithubDeviceStart,
  type GithubRepoView,
  type Grant,
  type GrantRequest,
  type PutCredentialRequest,
  type VaultCaller,
  type VaultConnectionView,
  type VaultProviderId,
  type VaultResult,
  type VaultRpc,
  type VaultStatusView
} from "./api";
import type { AuditEntry } from "./devices";
import type { Env } from "./env";
import { accountStub, devicesStub } from "./names";
import { installUrl, isRepoFullName } from "./providers/github-app";
import { fail, ok, type VaultFailure } from "./result";
import { isCaller, isFlowId, isGrantRequest, isId, isProvider, parseDeviceList } from "./validate";

export class Vault implements VaultRpc {
  constructor(private readonly env: Env) {}

  status(caller: VaultCaller): Promise<VaultResult<VaultStatusView>> {
    return this.run(caller, {}, async (userId) => {
      const disabled = await this.userDisabled(userId);
      if (disabled) return disabled;
      return this.view(userId);
    });
  }

  putCredential(
    caller: VaultCaller,
    provider: VaultProviderId,
    request: PutCredentialRequest
  ): Promise<VaultResult<VaultStatusView>> {
    return this.run(caller, {}, async (userId) => {
      if (!isProvider(provider)) return fail("bad_request", "unknown provider");
      if (provider === "github") return fail("bad_request", "github connects through the device flow");
      const material = (request as Partial<PutCredentialRequest> | null)?.material;
      if (typeof material !== "object" || material === null) return fail("bad_request", "material is required");
      const devices = parseDeviceList(request.authorizedDevices);
      if (!devices) return fail("bad_request", "authorizedDevices must be a list of device ids");
      const disabled = await this.userDisabled(userId);
      if (disabled) return disabled;
      const put = await accountStub(this.env, userId, provider).put(userId, provider, material, devices);
      if (!put.ok) return put;
      await this.audit(userId, { event: "upload", orgId: caller.orgId, provider, detail: `devices=${devices.length}` });
      return this.view(userId);
    });
  }

  authorize(
    caller: VaultCaller,
    provider: VaultProviderId,
    authorizedDevices: readonly string[]
  ): Promise<VaultResult<VaultStatusView>> {
    return this.run(caller, {}, async (userId) => {
      if (!isProvider(provider)) return fail("bad_request", "unknown provider");
      const devices = parseDeviceList(authorizedDevices);
      if (!devices) return fail("bad_request", "authorizedDevices must be a list of device ids");
      const disabled = await this.userDisabled(userId);
      if (disabled) return disabled;
      const result = await accountStub(this.env, userId, provider).authorize(userId, provider, devices);
      if (!result.ok) return result;
      await this.audit(userId, { event: "authorize", orgId: caller.orgId, provider, detail: devices.join(",") });
      return this.view(userId);
    });
  }

  disconnect(caller: VaultCaller, provider: VaultProviderId): Promise<VaultResult<VaultStatusView>> {
    return this.run(caller, {}, async (userId) => {
      if (!isProvider(provider)) return fail("bad_request", "unknown provider");
      const result = await accountStub(this.env, userId, provider).disconnect(userId, provider);
      if (!result.ok) return result;
      if (result.value.removed) await this.audit(userId, { event: "disconnect", orgId: caller.orgId, provider });
      return this.view(userId);
    });
  }

  enrollDevice(caller: VaultCaller, request: EnrollDeviceRequest): Promise<VaultResult<VaultStatusView>> {
    return this.run(caller, {}, async (userId) => {
      const body = request as Partial<EnrollDeviceRequest> | null;
      if (!body || !isId(body.deviceId) || (body.kind !== "laptop" && body.kind !== "cloud") || typeof body.publicKey !== "string") {
        return fail("bad_request", "expected {deviceId, kind: laptop|cloud, publicKey, parentId?}");
      }
      if (body.parentId !== undefined && !isId(body.parentId)) return fail("bad_request", "malformed parentId");
      const result = await devicesStub(this.env, userId).enroll(userId, caller.orgId, {
        deviceId: body.deviceId,
        kind: body.kind,
        publicKey: body.publicKey,
        ...(body.parentId !== undefined ? { parentId: body.parentId } : {})
      });
      if (!result.ok) return result;
      return this.view(userId);
    });
  }

  revokeDevice(caller: VaultCaller, deviceId: string): Promise<VaultResult<VaultStatusView>> {
    return this.run(caller, {}, async (userId) => {
      if (!isId(deviceId)) return fail("bad_request", "malformed deviceId");
      const result = await devicesStub(this.env, userId).revoke(userId, caller.orgId, deviceId);
      if (!result.ok) return result;
      return this.view(userId);
    });
  }

  setDisabled(caller: VaultCaller, disabled: boolean): Promise<VaultResult<VaultStatusView>> {
    return this.run(caller, {}, async (userId) => {
      if (typeof disabled !== "boolean") return fail("bad_request", "disabled must be a boolean");
      const result = await devicesStub(this.env, userId).setDisabled(userId, caller.orgId, disabled);
      if (!result.ok) return result;
      return this.view(userId);
    });
  }

  grant(caller: VaultCaller, request: GrantRequest): Promise<VaultResult<Grant>> {
    return this.run(caller, { runner: true }, async (userId) => {
      if (!isGrantRequest(request)) return fail("bad_request", "expected {provider, deviceId, ts, sig, repo?}");
      // A GitHub grant is an App installation token, and the repository
      // names which installation. Checked before the signature so the
      // device's `ts` isn't spent on a request that can't succeed.
      if (request.provider === "github" && request.repo === undefined) {
        return fail("bad_request", "github grants need the repository (owner/name) they are for");
      }
      if (caller.kind === "runner" && caller.deviceId !== request.deviceId) {
        return fail("forbidden", "runners may only request grants for their own device");
      }
      const devices = devicesStub(this.env, userId);
      // Also enforces the per-user kill switch and parent revocation, and
      // spends the `ts`.
      const verified = await devices.verifyGrant(userId, caller.orgId, {
        provider: request.provider,
        deviceId: request.deviceId,
        ts: request.ts,
        sig: request.sig
      });
      if (!verified.ok) return verified;
      const grant = await accountStub(this.env, userId, request.provider).grant(
        userId,
        request.provider,
        request.deviceId,
        verified.value.parentId,
        request.repo
      );
      await this.audit(userId, {
        event: grant.ok ? "grant" : "grant_denied",
        orgId: caller.orgId,
        deviceId: request.deviceId,
        provider: request.provider,
        detail: [grant.ok ? `generation ${grant.value.generation}` : grant.error, request.repo]
          .filter(Boolean)
          .join(" ")
      });
      return grant;
    });
  }

  githubDeviceStart(caller: VaultCaller, authorizedDevices: readonly string[]): Promise<VaultResult<GithubDeviceStart>> {
    return this.run(caller, {}, async (userId) => {
      const devices = parseDeviceList(authorizedDevices);
      if (!devices) return fail("bad_request", "authorizedDevices must be a list of device ids");
      const disabled = await this.userDisabled(userId);
      if (disabled) return disabled;
      return accountStub(this.env, userId, "github").githubStart(userId, devices);
    });
  }

  githubDevicePoll(caller: VaultCaller, flowId: string): Promise<VaultResult<GithubDevicePoll>> {
    return this.run(caller, {}, async (userId) => {
      if (!isFlowId(flowId)) return fail("not_found", "no such GitHub device flow");
      const disabled = await this.userDisabled(userId);
      if (disabled) return disabled;
      const result = await accountStub(this.env, userId, "github").githubPoll(userId, flowId);
      if (result.ok && result.value.state === "connected") {
        await this.audit(userId, { event: "github_connect", orgId: caller.orgId, provider: "github" });
      }
      return result;
    });
  }

  /** The repositories the user's GitHub connection reaches (users only), for
   * the add-project flow. Metadata only — the token stays in the vault. */
  githubRepos(caller: VaultCaller, query?: string): Promise<VaultResult<readonly GithubRepoView[]>> {
    return this.run(caller, {}, async (userId) => {
      if (query !== undefined && (typeof query !== "string" || query.length > 200)) {
        return fail("bad_request", "query must be a string of at most 200 characters");
      }
      const disabled = await this.userDisabled(userId);
      if (disabled) return disabled;
      return accountStub(this.env, userId, "github").githubRepos(userId, query);
    });
  }

  /** Branch names of `repo` (users only), for a new session's base branch. */
  githubBranches(caller: VaultCaller, repo: string): Promise<VaultResult<readonly string[]>> {
    return this.run(caller, {}, async (userId) => {
      if (!isRepoFullName(repo)) return fail("bad_request", "repo must be owner/name");
      const disabled = await this.userDisabled(userId);
      if (disabled) return disabled;
      return accountStub(this.env, userId, "github").githubBranches(userId, repo);
    });
  }

  // ── internals ────────────────────────────────────────────────────────────

  /** Steps 1–2 of the check order, plus a last-resort catch so a bug answers
   * `unavailable` instead of throwing across the service binding. */
  private async run<T>(
    caller: VaultCaller,
    options: { readonly runner?: boolean },
    body: (userId: string) => Promise<VaultResult<T>>
  ): Promise<VaultResult<T>> {
    if (this.env.VAULT_DISABLED === "1") return fail("disabled", "the vault is disabled", 503);
    if (!isCaller(caller)) return fail("bad_request", "malformed caller identity");
    if (caller.kind === "runner" && !options.runner) {
      return fail("forbidden", "runner tokens may only request grants");
    }
    try {
      return await body(caller.userId);
    } catch (error) {
      console.error(
        JSON.stringify({ event: "vault.error", message: error instanceof Error ? error.message : String(error) })
      );
      return fail("unavailable", "internal vault error", 500);
    }
  }

  private async userDisabled(userId: string): Promise<VaultFailure | undefined> {
    const result = await devicesStub(this.env, userId).isDisabled(userId);
    if (!result.ok) return result;
    return result.value ? fail("disabled", "the vault is disabled for this user", 403) : undefined;
  }

  private async view(userId: string): Promise<VaultResult<VaultStatusView>> {
    const [snapshot, ...views] = await Promise.all([
      devicesStub(this.env, userId).snapshot(userId),
      ...VAULT_PROVIDERS.map((provider) => accountStub(this.env, userId, provider).view(userId, provider))
    ]);
    if (!snapshot.ok) return snapshot;
    const connections: VaultConnectionView[] = [];
    for (const view of views) {
      if (!view.ok) return view;
      if (view.value) connections.push(view.value);
    }
    const slug = this.env.GITHUB_APP_SLUG;
    return ok({
      connections,
      devices: snapshot.value.devices,
      available: true,
      ...(slug ? { githubInstallUrl: installUrl(slug) } : {})
    });
  }

  private async audit(userId: string, entry: AuditEntry): Promise<void> {
    try {
      await devicesStub(this.env, userId).audit(userId, entry);
    } catch (error) {
      console.error(
        JSON.stringify({ event: "vault.audit.error", message: error instanceof Error ? error.message : String(error) })
      );
    }
  }
}
