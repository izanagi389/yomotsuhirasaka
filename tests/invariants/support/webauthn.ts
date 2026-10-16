// ドライバーの一部: ブラウザと認証器の代わりをする、ソフトウェアのパスキー（WebAuthn）。
// P-256（ES256）の鍵を作り、`none` の形式で登録し、署名で本人確認する。合否の判定はしない（作った値を返すだけ）。

import { createHash, generateKeyPairSync, randomBytes, sign, type KeyObject } from "node:crypto";

// --- 最小限のCBOR（登録に要る型だけ）---

type Cbor = number | string | Buffer | Map<number | string, Cbor>;

function head(major: number, n: number): Buffer {
  if (n < 24) return Buffer.from([(major << 5) | n]);
  if (n < 0x100) return Buffer.from([(major << 5) | 24, n]);
  if (n < 0x10000) return Buffer.from([(major << 5) | 25, n >> 8, n & 0xff]);
  const b = Buffer.alloc(5);
  b[0] = (major << 5) | 26;
  b.writeUInt32BE(n, 1);
  return b;
}

function cbor(value: Cbor): Buffer {
  if (typeof value === "number") return value >= 0 ? head(0, value) : head(1, -1 - value);
  if (typeof value === "string") {
    const bytes = Buffer.from(value, "utf8");
    return Buffer.concat([head(3, bytes.length), bytes]);
  }
  if (Buffer.isBuffer(value)) return Buffer.concat([head(2, value.length), value]);
  const parts: Buffer[] = [head(5, value.size)];
  for (const [k, v] of value) parts.push(cbor(k), cbor(v));
  return Buffer.concat(parts);
}

// --- 認証器 ---

const b64url = (b: Buffer): string => b.toString("base64url");
const sha256 = (b: Buffer | string): Buffer => createHash("sha256").update(b).digest();

export interface SoftPasskey {
  readonly credentialId: string;
  readonly privateKey: KeyObject;
  /** 登録のときの利用者のハンドル（base64url）。 */
  readonly userHandle: string;
  readonly rpId: string;
  readonly origin: string;
}

interface CreationOptions {
  publicKey: { challenge: string; rp: { id?: string }; user: { id: string } };
}

interface RequestOptions {
  publicKey: { challenge: string; rpId?: string };
}

const UP = 0x01;
const UV = 0x04;
const AT = 0x40;

function clientData(type: "webauthn.create" | "webauthn.get", challenge: string, origin: string): Buffer {
  return Buffer.from(JSON.stringify({ type, challenge, origin, crossOrigin: false }), "utf8");
}

/** `navigator.credentials.create()` の代わり。サーバーが返した登録の選択肢から、登録の応答を作る。 */
export function createCredential(options: CreationOptions, origin: string): { credential: unknown; passkey: SoftPasskey } {
  const rpId = options.publicKey.rp.id ?? new URL(origin).hostname;
  const { privateKey, publicKey } = generateKeyPairSync("ec", { namedCurve: "P-256" });
  const jwk = publicKey.export({ format: "jwk" }) as { x: string; y: string };
  const coseKey = cbor(
    new Map<number, Cbor>([
      [1, 2], // kty: EC2
      [3, -7], // alg: ES256
      [-1, 1], // crv: P-256
      [-2, Buffer.from(jwk.x, "base64url")],
      [-3, Buffer.from(jwk.y, "base64url")],
    ]),
  );
  const credentialId = randomBytes(32);
  const idLength = Buffer.alloc(2);
  idLength.writeUInt16BE(credentialId.length);
  const authData = Buffer.concat([
    sha256(rpId),
    Buffer.from([UP | UV | AT]),
    Buffer.alloc(4), // 署名の回数（使わない）
    Buffer.alloc(16), // AAGUID
    idLength,
    credentialId,
    coseKey,
  ]);
  const attestationObject = cbor(
    new Map<string, Cbor>([
      ["fmt", "none"],
      ["attStmt", new Map()],
      ["authData", authData],
    ]),
  );
  const id = b64url(credentialId);
  return {
    credential: {
      id,
      rawId: id,
      type: "public-key",
      response: {
        attestationObject: b64url(attestationObject),
        clientDataJSON: b64url(clientData("webauthn.create", options.publicKey.challenge, origin)),
      },
      extensions: {},
    },
    passkey: { credentialId: id, privateKey, userHandle: options.publicKey.user.id, rpId, origin },
  };
}

/** `navigator.credentials.get()` の代わり。サーバーが返した照合の選択肢に、パスキーで署名した応答を作る。 */
export function getAssertion(options: RequestOptions, passkey: SoftPasskey): unknown {
  const rpId = options.publicKey.rpId ?? passkey.rpId;
  const authData = Buffer.concat([sha256(rpId), Buffer.from([UP | UV]), Buffer.alloc(4)]);
  const client = clientData("webauthn.get", options.publicKey.challenge, passkey.origin);
  const signature = sign("sha256", Buffer.concat([authData, sha256(client)]), passkey.privateKey);
  return {
    id: passkey.credentialId,
    rawId: passkey.credentialId,
    type: "public-key",
    response: {
      authenticatorData: b64url(authData),
      clientDataJSON: b64url(client),
      signature: b64url(signature),
      userHandle: passkey.userHandle,
    },
    extensions: {},
  };
}
