// @zeron/proto — the Zeron wire surface in TypeScript.
//
// `generated/` is produced by wiregen (`cargo run -p wiregen`) from the Rust
// wire crates and is covered by a CI freshness gate; `shims.ts` is
// hand-written for the few shapes no Rust type pins down.
export * from "./generated/index";
export * from "./shims";
