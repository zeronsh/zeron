/** Durable Object naming. Both are versioned prefixes so a future layout can
 * live beside this one without colliding. */
import type { VaultProviderId } from "./api";
import type { Env } from "./env";

export const devicesName = (userId: string): string => `dev1/${userId}`;
export const accountName = (userId: string, provider: VaultProviderId): string => `acct1/${userId}/${provider}`;

export const devicesStub = (env: Env, userId: string) =>
  env.VAULT_DEVICES.get(env.VAULT_DEVICES.idFromName(devicesName(userId)));

export const accountStub = (env: Env, userId: string, provider: VaultProviderId) =>
  env.VAULT_ACCOUNTS.get(env.VAULT_ACCOUNTS.idFromName(accountName(userId, provider)));
