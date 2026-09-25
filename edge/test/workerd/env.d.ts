/// <reference types="@cloudflare/vitest-pool-workers" />

declare module "cloudflare:test" {
  interface ProvidedEnv {
    DEVICE_ROOMS: DurableObjectNamespace;
    TEST_LOG: DurableObjectNamespace;
    CHAT_ROOMS: DurableObjectNamespace;
    PREVIEW_ROOMS: DurableObjectNamespace;
    REGISTRY_ROOMS: DurableObjectNamespace;
  }
}
