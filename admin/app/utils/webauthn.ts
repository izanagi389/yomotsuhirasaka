// パスキー（WebAuthn）。サーバー（webauthn-rs）は、バイト列を base64url の文字列で送り受けする。
// ブラウザの navigator.credentials は ArrayBuffer を使うので、ここで変換する（ライブラリは使わない）。

function fromB64url(s: string): ArrayBuffer {
  const b64 = s.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(s.length / 4) * 4, "=");
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out.buffer;
}

function toB64url(buf: ArrayBuffer | null): string | null {
  if (!buf) return null;
  const bytes = new Uint8Array(buf);
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

type Json = Record<string, unknown>;

/** サーバーの `CreationChallengeResponse` で新しいパスキーを作り、`RegisterPublicKeyCredential` の形にする。 */
export async function createPasskey(challenge: { publicKey: Json }): Promise<Json> {
  const pk = challenge.publicKey as Json & { challenge: string; user: Json & { id: string }; excludeCredentials?: (Json & { id: string })[] };
  const options = {
    ...pk,
    challenge: fromB64url(pk.challenge),
    user: { ...pk.user, id: fromB64url(pk.user.id) },
    excludeCredentials: (pk.excludeCredentials ?? []).map((c) => ({ ...c, id: fromB64url(c.id) })),
  } as unknown as PublicKeyCredentialCreationOptions;
  const cred = (await navigator.credentials.create({ publicKey: options })) as PublicKeyCredential | null;
  if (!cred) throw new Error("passkey_cancelled");
  const res = cred.response as AuthenticatorAttestationResponse;
  return {
    id: cred.id,
    rawId: toB64url(cred.rawId),
    type: cred.type,
    response: {
      attestationObject: toB64url(res.attestationObject),
      clientDataJSON: toB64url(res.clientDataJSON),
      transports: typeof res.getTransports === "function" ? res.getTransports() : undefined,
    },
    extensions: cred.getClientExtensionResults(),
  };
}

/** サーバーの `RequestChallengeResponse` でパスキーを使い、`PublicKeyCredential` の形にする。 */
export async function usePasskey(challenge: { publicKey: Json }): Promise<Json> {
  const pk = challenge.publicKey as Json & { challenge: string; allowCredentials?: (Json & { id: string })[] };
  const options = {
    ...pk,
    challenge: fromB64url(pk.challenge),
    allowCredentials: (pk.allowCredentials ?? []).map((c) => ({ ...c, id: fromB64url(c.id) })),
  } as unknown as PublicKeyCredentialRequestOptions;
  const cred = (await navigator.credentials.get({ publicKey: options })) as PublicKeyCredential | null;
  if (!cred) throw new Error("passkey_cancelled");
  const res = cred.response as AuthenticatorAssertionResponse;
  return {
    id: cred.id,
    rawId: toB64url(cred.rawId),
    type: cred.type,
    response: {
      authenticatorData: toB64url(res.authenticatorData),
      clientDataJSON: toB64url(res.clientDataJSON),
      signature: toB64url(res.signature),
      userHandle: toB64url(res.userHandle),
    },
    extensions: cred.getClientExtensionResults(),
  };
}
