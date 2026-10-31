/** Helpers shared by the Node unit tier and the workerd tier (WebCrypto only). */
import type { VaultProviderId } from "../src/api";
import { toBase64Url, utf8 } from "../src/encoding";

/** Unsigned JWT-shaped token: the vault only ever reads claims, never verifies. */
export const fakeJwt = (claims: Record<string, unknown>): string =>
  `${toBase64Url(utf8(JSON.stringify({ alg: "RS256" })))}.${toBase64Url(utf8(JSON.stringify(claims)))}.c2ln`;

export interface TestDevice {
  readonly deviceId: string;
  /** Raw Ed25519 public key, base64url. */
  readonly publicKey: string;
  sign(message: string): Promise<string>;
  /** Signature over the vault grant statement. */
  grantSig(userId: string, provider: VaultProviderId, ts: number): Promise<string>;
}

export const makeDevice = async (deviceId: string): Promise<TestDevice> => {
  const pair = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"])) as CryptoKeyPair;
  const raw = new Uint8Array((await crypto.subtle.exportKey("raw", pair.publicKey)) as ArrayBuffer);
  const sign = async (message: string) =>
    toBase64Url(new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, pair.privateKey, utf8(message))));
  return {
    deviceId,
    publicKey: toBase64Url(raw),
    sign,
    grantSig: (userId, provider, ts) => sign(`zeron-vault-grant\n${userId}\n${deviceId}\n${provider}\n${ts}`)
  };
};
