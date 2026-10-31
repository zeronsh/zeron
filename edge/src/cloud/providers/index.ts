/**
 * Sandbox provider registry. `SANDBOX_PROVIDER` (default "boat") picks the
 * provider NEW sandboxes are created on; existing sandboxes keep using the
 * provider recorded with them, so switching the var migrates users as they
 * re-provision while billing for the old sandboxes continues exactly.
 */
import type { Env } from "../../env";
import type { SandboxProvider } from "../sandbox-provider";
import { BoatProvider } from "./boat";
import { FakeProvider } from "./fake";

export type ProviderEnv = Pick<Env, "SANDBOX_PROVIDER" | "BOAT_API_KEY" | "BOAT_API_BASE">;

export const DEFAULT_PROVIDER = "boat";

// One fake per isolate, so a dev worker's DO and workflows (one isolate
// under `wrangler dev`) see the same machines.
let fake: FakeProvider | undefined;

/** The provider for NEW sandboxes. */
export const activeProviderName = (env: ProviderEnv): string => env.SANDBOX_PROVIDER?.trim() || DEFAULT_PROVIDER;

/** A configured provider by name; undefined when it has no credentials here. */
export const sandboxProvider = (env: ProviderEnv, name = activeProviderName(env)): SandboxProvider | undefined => {
  switch (name) {
    case "boat":
      return env.BOAT_API_KEY ? new BoatProvider({ apiKey: env.BOAT_API_KEY, base: env.BOAT_API_BASE }) : undefined;
    case "fake":
      // Only when explicitly selected: never a silent fallback in production.
      return activeProviderName(env) === "fake" ? (fake ??= new FakeProvider()) : undefined;
    default:
      return undefined;
  }
};

/** Every provider this deployment can reach (the orphan scan walks them all). */
export const configuredProviders = (env: ProviderEnv): SandboxProvider[] =>
  ["boat", "fake"].flatMap((name) => sandboxProvider(env, name) ?? []);
