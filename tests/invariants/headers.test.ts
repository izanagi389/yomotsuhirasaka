// 応答のヘッダーと公開の範囲の約束: INV-HDR-01, INV-HDR-02, INV-CFG-02

import { execFileSync } from "node:child_process";
import { expect } from "vitest";
import { inv } from "./support/inv.js";
import { asSession, driver } from "./support/driver.js";
import type { HttpResponse } from "./support/http.js";
import { lazyWorld } from "./support/world.js";

const world = lazyWorld();

function cspDirectives(res: HttpResponse, what: string): Map<string, string[]> {
  const header = res.headers.get("content-security-policy");
  expect(header, `${what} に Content-Security-Policy がない`).toBeTruthy();
  const map = new Map<string, string[]>();
  for (const part of header!.split(";")) {
    const [name, ...values] = part.trim().split(/\s+/);
    if (name) map.set(name.toLowerCase(), values);
  }
  return map;
}

function expectSecurityHeaders(res: HttpResponse, what: string): void {
  const csp = cspDirectives(res, what);
  const all = [...csp.values()].flat().map((v) => v.toLowerCase());
  expect(all, `${what} のCSPが 'unsafe-inline' を許している`).not.toContain("'unsafe-inline'");
  expect(all, `${what} のCSPが 'unsafe-eval' を許している`).not.toContain("'unsafe-eval'");
  expect(csp.get("frame-ancestors"), `${what} のCSPの frame-ancestors`).toEqual(["'none'"]);

  const hsts = res.headers.get("strict-transport-security");
  expect(hsts, `${what} に Strict-Transport-Security がない`).toBeTruthy();
  const maxAge = Number(/max-age=(\d+)/i.exec(hsts!)?.[1] ?? "0");
  expect(maxAge, `${what} のHSTSの max-age`).toBeGreaterThan(0);

  expect(res.headers.get("x-content-type-options")?.toLowerCase(), `${what} の X-Content-Type-Options`).toBe("nosniff");
}

inv(
  "inv_hdr_01",
  "管理画面と管理APIの応答には、CSPとHSTSなどのセキュリティヘッダーが付く。CSPは `unsafe-inline` と `unsafe-eval` を許さず、`frame-ancestors 'none'` を含む",
  async () => {
    const { yomi, a } = await world();
    expectSecurityHeaders(await yomi.adminUiRaw("/"), "管理画面のトップ");
    expectSecurityHeaders(await yomi.adminUiRaw("/workspaces/some/deep/route"), "管理画面の深いパス");
    expectSecurityHeaders(await yomi.meRaw(asSession(a.adminSession)), "管理APIの成功の応答");
    expectSecurityHeaders(await yomi.meRaw({ kind: "none" }), "管理APIの拒否の応答");
  },
);

inv("inv_hdr_02", "配信APIと管理APIは、別のドメインからのアクセスを許すCORSのヘッダーを返さない", async () => {
  const { yomi, a } = await world();
  const origin = "https://evil.inv.test";
  for (const kind of ["adminApi", "deliveryApi"] as const) {
    for (const method of ["GET", "OPTIONS"] as const) {
      const res = await yomi.corsProbeRaw(kind, method, origin, a.id);
      const allowOrigin = res.headers.get("access-control-allow-origin");
      const what = `${kind} への ${method}（Origin: ${origin}）`;
      expect(allowOrigin === "*" || allowOrigin === origin, `${what} が別のドメインを許している: ${allowOrigin}`).toBe(false);
      expect(res.headers.get("access-control-allow-credentials"), `${what} が資格情報付きのアクセスを許している`).not.toBe("true");
    }
  }
});

inv("inv_cfg_02", "公式のDocker Composeで起動したとき、PostgreSQLのポートはホストの外に開かれていない", async () => {
  const file = driver.officialComposeFile();
  // docker compose で、変数や上書きを解決した後の構成を読む
  const json = execFileSync("docker", ["compose", "-f", file, "config", "--format", "json"], { encoding: "utf8" });
  const config = JSON.parse(json) as { services: Record<string, { image?: string; ports?: unknown[]; network_mode?: string }> };
  const dbServices = Object.entries(config.services).filter(([name, s]) => /postgres/i.test(s.image ?? "") || /^(db|postgres|database)$/i.test(name));
  expect(dbServices.length, "公式のDocker ComposeにPostgreSQLのサービスが見つからない").toBeGreaterThan(0);
  for (const [name, service] of dbServices) {
    expect(service.ports ?? [], `${name} がポートを公開している`).toEqual([]);
    expect(service.network_mode, `${name} がホストのネットワークを使っている`).not.toBe("host");
  }
});
