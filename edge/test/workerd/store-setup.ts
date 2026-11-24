import { applyD1Migrations, env } from "cloudflare:test";

// The store's schema (migrations/store), as the deploy job applies it before
// the worker deploys. Idempotent: applied migrations are recorded.
await applyD1Migrations(env.STORE_DB, env.STORE_MIGRATIONS);
