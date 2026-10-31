import { describe, expect, it } from "vitest";
import { SandboxError, classifyFailure, effectiveTtl, sandboxUserMessage, stopRetryable } from "./sandbox-provider";

const err = (kind: SandboxError["kind"], message = "detail", action?: SandboxError["action"]) =>
  new SandboxError(kind, message, "code", "p", action);

describe("provider-neutral failure handling", () => {
  it("retries only retryable provider errors (and non-provider hiccups)", () => {
    expect(classifyFailure(err("retryable"), "x").retryable).toBe(true);
    expect(classifyFailure(new Error("socket hang up"), "check the Cloud sandbox")).toEqual({
      retryable: true,
      message: "Couldn't check the Cloud sandbox: socket hang up"
    });
    for (const kind of ["config", "notFound", "conflict", "fatal"] as const) {
      expect(classifyFailure(err(kind), "x").retryable).toBe(false);
    }
  });

  it("phrases config and not-found failures for the user", () => {
    expect(sandboxUserMessage(err("config", "the key lacks sandbox.create", "create"), "x")).toBe(
      "Cloud isn't configured to create sandboxes (the key lacks sandbox.create)."
    );
    expect(sandboxUserMessage(err("config", "the key was rejected"), "x")).toBe("Cloud is misconfigured (the key was rejected).");
    expect(sandboxUserMessage(err("notFound"), "x")).toBe("The Cloud sandbox no longer exists. Delete Cloud and enable it again.");
    expect(sandboxUserMessage(err("fatal", "at capacity"), "create the Cloud sandbox")).toBe(
      "Couldn't create the Cloud sandbox: at capacity"
    );
  });

  it("keeps retrying a refused stop, but not a forbidden or vanished one", () => {
    expect(stopRetryable(err("conflict"))).toBe(true);
    expect(stopRetryable(err("retryable"))).toBe(true);
    expect(stopRetryable(err("fatal"))).toBe(true);
    expect(stopRetryable(err("config"))).toBe(false);
    expect(stopRetryable(err("notFound"))).toBe(false);
  });

  it("clamps the configured TTL to the provider's capabilities", () => {
    expect(effectiveTtl({ ttlRequired: false, maxTtlSeconds: null }, null)).toBeNull();
    expect(effectiveTtl({ ttlRequired: false, maxTtlSeconds: 2_592_000 }, 7200)).toBe(7200);
    expect(effectiveTtl({ ttlRequired: false, maxTtlSeconds: 3600 }, 7200)).toBe(3600);
    expect(effectiveTtl({ ttlRequired: true, maxTtlSeconds: 7200 }, null)).toBe(7200);
  });
});
