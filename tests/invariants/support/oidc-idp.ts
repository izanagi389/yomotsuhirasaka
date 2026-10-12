// 擬似IdP（OIDC）。認可エンドポイントは利用者の操作なしに、次に設定したクレームでコードを発行して戻す。
// IDトークンはRS256で署名する（鍵は起動のたびに作る）。INV-AUTH-19 などで使う。

import { createHash, createPublicKey, generateKeyPairSync, randomBytes, sign, type KeyObject } from "node:crypto";
import { createServer, type IncomingMessage, type Server } from "node:http";
import { readFileSync } from "node:fs";
import { createServer as createHttpsServer } from "node:https";

export interface IdpClaims {
  sub: string;
  email: string;
  /** undefined ならクレーム自体を含めない。 */
  email_verified?: boolean;
}

export interface IdpAuthorizeRequest {
  params: Record<string, string>;
}

export interface MockIdp {
  readonly issuer: string;
  /** 次のログインで使うクレーム。 */
  setNextLogin(claims: IdpClaims): void;
  readonly authorizeRequests: IdpAuthorizeRequest[];
  /** トークンの要求ごとに、PKCEの検証が通ったか。 */
  readonly tokenRequests: { pkceVerified: boolean; clientAuthenticated: boolean }[];
  close(): Promise<void>;
}

const b64url = (b: Buffer | string): string => Buffer.from(b).toString("base64url");

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve) => {
    const chunks: Buffer[] = [];
    req.on("data", (c: Buffer) => chunks.push(c));
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
  });
}

export function signIdToken(privateKey: KeyObject, kid: string, payload: Record<string, unknown>): string {
  const header = b64url(JSON.stringify({ alg: "RS256", typ: "JWT", kid }));
  const body = b64url(JSON.stringify(payload));
  const signature = sign("RSA-SHA256", Buffer.from(`${header}.${body}`), privateKey);
  return `${header}.${body}.${b64url(signature)}`;
}

export async function startOidcIdp(options: {
  port: number;
  issuer: string;
  clientId: string;
  clientSecret: string;
  tls?: { certFile: string; keyFile: string };
}): Promise<MockIdp> {
  const { privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const kid = randomBytes(8).toString("hex");
  const jwk = { ...createPublicKey(privateKey).export({ format: "jwk" }), kid, use: "sig", alg: "RS256" };
  const issuer = options.issuer.replace(/\/$/, "");

  let next: IdpClaims | null = null;
  const codes = new Map<string, { claims: IdpClaims; nonce?: string; challenge?: string; redirectUri: string }>();
  const authorizeRequests: IdpAuthorizeRequest[] = [];
  const tokenRequests: { pkceVerified: boolean; clientAuthenticated: boolean }[] = [];

  const handler = async (req: IncomingMessage, res: import("node:http").ServerResponse) => {
    const url = new URL(req.url ?? "/", issuer);
    const json = (status: number, value: unknown) => {
      res.writeHead(status, { "content-type": "application/json" });
      res.end(JSON.stringify(value));
    };

    if (url.pathname === "/.well-known/openid-configuration") {
      return json(200, {
        issuer,
        authorization_endpoint: `${issuer}/authorize`,
        token_endpoint: `${issuer}/token`,
        jwks_uri: `${issuer}/jwks`,
        response_types_supported: ["code"],
        subject_types_supported: ["public"],
        id_token_signing_alg_values_supported: ["RS256"],
        code_challenge_methods_supported: ["S256"],
        token_endpoint_auth_methods_supported: ["client_secret_basic", "client_secret_post"],
        scopes_supported: ["openid", "email", "profile"],
        claims_supported: ["sub", "email", "email_verified"],
      });
    }
    if (url.pathname === "/jwks") return json(200, { keys: [jwk] });

    if (url.pathname === "/authorize") {
      const params = Object.fromEntries(url.searchParams.entries());
      authorizeRequests.push({ params });
      if (!next || params.client_id !== options.clientId || !params.redirect_uri) {
        res.writeHead(400).end("bad authorize request");
        return;
      }
      const code = randomBytes(16).toString("hex");
      codes.set(code, { claims: next, nonce: params.nonce, challenge: params.code_challenge, redirectUri: params.redirect_uri });
      const back = new URL(params.redirect_uri);
      back.searchParams.set("code", code);
      if (params.state !== undefined) back.searchParams.set("state", params.state);
      res.writeHead(302, { location: back.toString() }).end();
      return;
    }

    if (url.pathname === "/token" && req.method === "POST") {
      const form = new URLSearchParams(await readBody(req));
      let clientId = form.get("client_id");
      let clientSecret = form.get("client_secret");
      const basic = req.headers.authorization?.match(/^Basic (.+)$/i);
      if (basic) {
        const [id, secret] = Buffer.from(basic[1]!, "base64").toString("utf8").split(":");
        clientId = decodeURIComponent(id ?? "");
        clientSecret = decodeURIComponent(secret ?? "");
      }
      const clientAuthenticated = clientId === options.clientId && clientSecret === options.clientSecret;
      const entry = codes.get(form.get("code") ?? "");
      const verifier = form.get("code_verifier") ?? "";
      const pkceVerified = !!entry?.challenge && b64url(createHash("sha256").update(verifier).digest()) === entry.challenge;
      tokenRequests.push({ pkceVerified, clientAuthenticated });
      if (!entry || !clientAuthenticated || form.get("redirect_uri") !== entry.redirectUri) {
        return json(400, { error: "invalid_grant" });
      }
      codes.delete(form.get("code") ?? "");
      const now = Math.floor(Date.now() / 1000);
      const payload: Record<string, unknown> = {
        iss: issuer,
        sub: entry.claims.sub,
        aud: options.clientId,
        iat: now,
        exp: now + 300,
        email: entry.claims.email,
      };
      if (entry.claims.email_verified !== undefined) payload.email_verified = entry.claims.email_verified;
      if (entry.nonce !== undefined) payload.nonce = entry.nonce;
      return json(200, {
        access_token: randomBytes(16).toString("hex"),
        token_type: "Bearer",
        expires_in: 300,
        id_token: signIdToken(privateKey, kid, payload),
      });
    }

    res.writeHead(404).end();
  };

  const listener = (req: IncomingMessage, res: import("node:http").ServerResponse) => {
    handler(req, res).catch(() => res.writeHead(500).end());
  };
  const server: Server = options.tls
    ? createHttpsServer({ cert: readFileSync(options.tls.certFile), key: readFileSync(options.tls.keyFile) }, listener)
    : createServer(listener);
  await new Promise<void>((resolve) => server.listen(options.port, "0.0.0.0", resolve));

  return {
    issuer,
    setNextLogin: (claims) => {
      next = claims;
    },
    authorizeRequests,
    tokenRequests,
    close: () =>
      new Promise((resolve) => {
        server.closeAllConnections?.();
        server.close(() => resolve());
      }),
  };
}
