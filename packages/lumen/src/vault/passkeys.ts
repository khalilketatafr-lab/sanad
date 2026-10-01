/**
 * Passkey sign-up and sign-in (blueprint 01 §3.2). Runs on the MAIN thread:
 * `navigator.credentials` does not exist in workers. The Kernel ceremony is
 * DPoP-bound to the device, so the passkey signs the device in, never just a
 * browser session.
 *
 * Options and responses travel as WebAuthn's JSON forms; binary members are
 * base64url. The conversions are explicit (not `parseCreationOptionsFromJSON`
 * / `toJSON()`), so every engine the reader supports behaves the same.
 */
import { b64url, b64urlDecode } from "./jose.ts";
import type { KernelClient, PasskeySession } from "./kernel-client.ts";

type Json = Record<string, unknown>;

const bytes = (v: unknown): Uint8Array<ArrayBuffer> => b64urlDecode(String(v));

function creationOptions(o: Json): PublicKeyCredentialCreationOptions {
  const user = o["user"] as Json;
  return {
    ...(o as unknown as PublicKeyCredentialCreationOptions),
    challenge: bytes(o["challenge"]),
    user: { id: bytes(user["id"]), name: String(user["name"]), displayName: String(user["displayName"]) },
    excludeCredentials: ((o["excludeCredentials"] as Json[] | undefined) ?? []).map((c) => ({ type: "public-key", id: bytes(c["id"]) })),
  };
}

function requestOptions(o: Json): PublicKeyCredentialRequestOptions {
  return {
    ...(o as unknown as PublicKeyCredentialRequestOptions),
    challenge: bytes(o["challenge"]),
    allowCredentials: ((o["allowCredentials"] as Json[] | undefined) ?? []).map((c) => ({ type: "public-key", id: bytes(c["id"]) })),
  };
}

function credentialJson(c: PublicKeyCredential): Json {
  const r = c.response;
  const response: Json = { clientDataJSON: b64url(r.clientDataJSON) };
  if (r instanceof AuthenticatorAttestationResponse) {
    response["attestationObject"] = b64url(r.attestationObject);
    response["transports"] = r.getTransports();
  } else if (r instanceof AuthenticatorAssertionResponse) {
    response["authenticatorData"] = b64url(r.authenticatorData);
    response["signature"] = b64url(r.signature);
    if (r.userHandle !== null) response["userHandle"] = b64url(r.userHandle);
  }
  return { id: c.id, rawId: b64url(c.rawId), type: c.type, response, clientExtensionResults: c.getClientExtensionResults() };
}

/** Creates an account whose first passkey lives in this device's authenticator. */
export async function signUpWithPasskey(client: KernelClient, name?: string): Promise<PasskeySession> {
  const { ceremony, publicKey } = await client.passkeyBegin("register", name);
  const cred = await navigator.credentials.create({ publicKey: creationOptions(publicKey) });
  if (!(cred instanceof PublicKeyCredential)) throw new Error("passkey creation was cancelled");
  return client.passkeyFinish(ceremony, credentialJson(cred));
}

/** Signs this device in with any passkey for our RP (discoverable: no username). */
export async function signInWithPasskey(client: KernelClient): Promise<PasskeySession> {
  const { ceremony, publicKey } = await client.passkeyBegin("authenticate");
  const cred = await navigator.credentials.get({ publicKey: requestOptions(publicKey) });
  if (!(cred instanceof PublicKeyCredential)) throw new Error("passkey sign-in was cancelled");
  return client.passkeyFinish(ceremony, credentialJson(cred));
}
