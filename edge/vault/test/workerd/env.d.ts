/// <reference types="@cloudflare/vitest-pool-workers/types" />
import type { Env as VaultEnv } from "../../src/env";

declare global {
  namespace Cloudflare {
    interface Env extends VaultEnv {
      /** SPKI PEM of the per-run test GitHub App key (vitest.workerd.config.ts). */
      readonly TEST_GITHUB_APP_PUBLIC_KEY: string;
    }
    interface GlobalProps {
      mainModule: typeof import("../../src/index");
    }
  }
}
