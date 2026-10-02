/// <reference types="@cloudflare/vitest-pool-workers" />

declare module "cloudflare:test" {
  interface ProvidedEnv {
    DEVICE_ROOMS: DurableObjectNamespace;
    TEST_LOG: DurableObjectNamespace;
    CHAT_ROOMS: DurableObjectNamespace;
    PREVIEW_ROOMS: DurableObjectNamespace;
    REGISTRY_ROOMS: DurableObjectNamespace;
    CLOUD_ACCOUNTS: DurableObjectNamespace<import("../../src/cloud/cloud-account").CloudAccount>;
    CLOUD_INDEX: DurableObjectNamespace<import("../../src/cloud/cloud-index").CloudIndex>;
    CLOUD_PROVISION: Workflow;
    CLOUD_WAKE: Workflow;
    CLOUD_SLEEP: Workflow;
    CLOUD_DELETE: Workflow;
    VAULT: { calls(): Promise<{ method: string; args: unknown[] }[]> };
    BLOBS: R2Bucket;
    AUTH_MODE: string;
    ADMIN_TOKEN: string;
    RUNNER_JWT_PUBLIC_KEY: string;
    RUNNER_JWT_PRIVATE_KEY: string;
  }
}
