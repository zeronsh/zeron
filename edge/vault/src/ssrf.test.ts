import { describe, expect, it } from "vitest";
import { assertSafeHttpsUrl, UnsafeUrlError } from "./ssrf";

describe("assertSafeHttpsUrl", () => {
  it("accepts valid public HTTPS URLs", () => {
    expect(assertSafeHttpsUrl("https://mcp.linear.app/sse", "url").href).toBe("https://mcp.linear.app/sse");
    expect(assertSafeHttpsUrl("https://auth.example.com/oauth/token", "tokenEndpoint").href).toBe(
      "https://auth.example.com/oauth/token"
    );
    expect(assertSafeHttpsUrl("https://auth.example.com:443/oauth/token", "tokenEndpoint").href).toBe(
      "https://auth.example.com/oauth/token"
    );
    expect(assertSafeHttpsUrl("https://sub-1.api.example.co.uk/v1/mcp?foo=bar", "url").hostname).toBe(
      "sub-1.api.example.co.uk"
    );
    expect(assertSafeHttpsUrl("https://mcp.example.com:8443/mcp", "url", { allowCustomPort: true }).port).toBe("8443");
  });

  it("rejects non-HTTPS protocols", () => {
    for (const raw of [
      "http://example.com/token",
      "ftp://example.com/file",
      "file:///etc/passwd",
      "javascript:alert(1)",
      "ws://example.com/mcp",
      "wss://example.com/mcp"
    ]) {
      expect(() => assertSafeHttpsUrl(raw, "tokenEndpoint"), raw).toThrow(UnsafeUrlError);
    }
  });

  it("rejects embedded credentials", () => {
    for (const raw of [
      "https://user:pass@example.com/token",
      "https://user@example.com/token",
      "https://:pass@example.com/token"
    ]) {
      expect(() => assertSafeHttpsUrl(raw, "tokenEndpoint"), raw).toThrow(UnsafeUrlError);
    }
  });

  it("rejects non-443 ports by default (tokenEndpoint)", () => {
    for (const raw of [
      "https://auth.example.com:8443/token",
      "https://auth.example.com:80/token",
      "https://auth.example.com:3000/token",
      "https://auth.example.com:6379/token"
    ]) {
      expect(() => assertSafeHttpsUrl(raw, "tokenEndpoint"), raw).toThrow(UnsafeUrlError);
    }
  });

  it("rejects IPv4 literals including decimal, hex, octal, and shorthand forms", () => {
    for (const raw of [
      "https://127.0.0.1/token",
      "https://10.0.0.1/token",
      "https://192.168.1.1/token",
      "https://172.16.0.1/token",
      "https://169.254.169.254/latest/meta-data/",
      "https://100.100.100.200/latest/meta-data/",
      "https://8.8.8.8/token",
      "https://2130706433/token",
      "https://0x7f000001/token",
      "https://0x7f.0x0.0x0.0x1/token",
      "https://0177.0.0.1/token",
      "https://017700000001/token",
      "https://127.1/token",
      "https://127.0.1/token",
      "https://169.254.169.254./token"
    ]) {
      expect(() => assertSafeHttpsUrl(raw, "tokenEndpoint"), raw).toThrow(UnsafeUrlError);
    }
  });

  it("rejects IPv6 literals", () => {
    for (const raw of [
      "https://[::1]/token",
      "https://[::ffff:127.0.0.1]/token",
      "https://[::ffff:169.254.169.254]/token",
      "https://[fe80::1]/token",
      "https://[2606:4700:4700::1111]/token"
    ]) {
      expect(() => assertSafeHttpsUrl(raw, "tokenEndpoint"), raw).toThrow(UnsafeUrlError);
    }
  });

  it("rejects localhost, .localhost, .local, .internal, .arpa, and single-label hosts", () => {
    for (const raw of [
      "https://localhost/token",
      "https://LOCALHOST/token",
      "https://localhost./token",
      "https://sub.localhost/token",
      "https://foo.bar.localhost/token",
      "https://myprinter.local/token",
      "https://myprinter.local./token",
      "https://metadata.google.internal/computeMetadata/v1/",
      "https://service.corp.internal/token",
      "https://1.0.0.127.in-addr.arpa/token",
      "https://home.arpa/token",
      "https://metadata/token",
      "https://intranet/token"
    ]) {
      expect(() => assertSafeHttpsUrl(raw, "tokenEndpoint"), raw).toThrow(UnsafeUrlError);
    }
  });

  it("rejects malformed strings, control characters, and whitespace", () => {
    for (const raw of [
      "",
      "not-a-url",
      " https://example.com/token",
      "https://example.com/token ",
      "https://example.com/token\r\nX-Injected: 1",
      "https://.example.com/token",
      "https://example..com/token",
      "https://example.com/" + "a".repeat(2100)
    ]) {
      expect(() => assertSafeHttpsUrl(raw, "tokenEndpoint"), raw).toThrow(UnsafeUrlError);
    }
  });
});
