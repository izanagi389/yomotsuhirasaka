// CMSのリダイレクト一覧（配信APIの redirects）を、サイトのパスの規則にする（05-urls-and-redirects.md、Q40, Q145）。
// ここは Nitro に依存しない（テストで直接呼ぶ）。
//
// - 扱うのは、転送元がサイトのURL（siteUrl）と同じオリジンのものだけ。他のオリジンの規則は、そのサイトの担当なので使わない
// - 転送先が同じオリジンならパスに、違えば完全なURLのままにする
// - 410（記事を削除した）は、転送先なしの「消えた」として持つ

export interface YomiRedirect {
  from: string;
  to: string | null;
  status: number;
}

export interface PathRule {
  /** 301 か 410。 */
  status: number;
  /** 転送先（301 のとき）。同じオリジンならパス。 */
  to: string | null;
}

/** パス（とクエリ）の比べる形: 末尾の / を除く（ルートは / のまま）。 */
export function normalizePath(path: string): string {
  const p = path.split("#")[0] ?? "";
  return p.length > 1 ? p.replace(/\/+$/, "") || "/" : p;
}

/** リダイレクト一覧を、サイトのパス → 規則 にする。 */
export function toPathRules(redirects: YomiRedirect[], siteUrl: string): Map<string, PathRule> {
  const site = new URL(siteUrl);
  const out = new Map<string, PathRule>();
  for (const r of redirects) {
    let from: URL;
    try {
      from = new URL(r.from);
    } catch {
      continue;
    }
    if (from.origin !== site.origin) continue;
    const key = normalizePath(from.pathname + from.search);
    if (r.status === 410 || r.to === null) {
      out.set(key, { status: 410, to: null });
      continue;
    }
    let to: URL;
    try {
      to = new URL(r.to);
    } catch {
      continue;
    }
    if (to.protocol !== "https:" && to.protocol !== "http:") continue;
    out.set(key, { status: 301, to: to.origin === site.origin ? to.pathname + to.search : to.toString() });
  }
  return out;
}

/** routeRules のパターンとして読まれる文字。 */
const ROUTE_PATTERN = /[*:()[\]{}]/;

/** Nuxt の routeRules（ビルドのときに作る 301）。410 は routeRules では表せないので、サーバーのミドルウェアで返す。 */
export function toRouteRules(rules: Map<string, PathRule>): Record<string, { redirect: { to: string; statusCode: number } }> {
  const out: Record<string, { redirect: { to: string; statusCode: number } }> = {};
  for (const [path, rule] of rules) {
    // routeRules のキーはパターンとして読まれる（`/**`、`:name` など）。パターンの文字を含むパスは、
    // 1つのパスの規則にならないので入れない（ミドルウェアは文字どおりに照合するので、そちらで返す）
    if (rule.status === 301 && rule.to && !path.includes("?") && !ROUTE_PATTERN.test(path)) out[path] = { redirect: { to: rule.to, statusCode: 301 } };
  }
  return out;
}

/** Nuxt のページ（`pages:extend` で渡る形の一部）。 */
export interface PageLike {
  path: string;
  children?: PageLike[];
}

/**
 * サイト自身の固定のページのパス（パラメーターを持たないルート）。CMS のリダイレクトは、これらのパスには当てない
 * （取り込みの旧URLなどで、サイトにもともとある CMS の外のページを 301 で奪わせない。第三者の確認の R26）。
 */
export function staticPagePaths(pages: PageLike[], parent = ""): string[] {
  const out: string[] = [];
  for (const page of pages) {
    const path = page.path.startsWith("/") ? page.path : `${parent.replace(/\/+$/, "")}/${page.path}`;
    if (!ROUTE_PATTERN.test(path)) out.push(canonicalPath(path || "/"));
    if (page.children) out.push(...staticPagePaths(page.children, path));
  }
  return out;
}

/** 比べる形: URL のパスと同じエンコードにし、末尾の / を除く。 */
export function canonicalPath(path: string): string {
  let p: string;
  try {
    p = new URL(path, "http://x").pathname;
  } catch {
    p = path;
  }
  return normalizePath(p);
}

/** 守るパスの一覧（完全一致）と、守る先頭（その下をすべて守る）。`baseURL` はサイトの `app.baseURL`。 */
export interface Protection {
  paths: string[];
  prefixes: string[];
  baseURL: string;
}

/** パスが守る対象か（`baseURL` を除いた形でも比べる）。 */
export function isProtected(path: string, p: Protection): boolean {
  const canon = canonicalPath(path.split("?")[0] ?? path);
  const base = normalizePath(p.baseURL || "/");
  const candidates = [canon];
  if (base !== "/" && (canon === base || canon.startsWith(`${base}/`))) candidates.push(canonicalPath(canon.slice(base.length) || "/"));
  return candidates.some((c) => p.paths.includes(c) || p.prefixes.some((pre) => c === pre || c.startsWith(`${pre}/`)));
}

/** 守るパスを除いた規則。 */
export function withoutProtected(rules: Map<string, PathRule>, protection: Protection): Map<string, PathRule> {
  const out = new Map<string, PathRule>();
  for (const [key, rule] of rules) {
    if (!isProtected(key, protection)) out.set(key, rule);
  }
  return out;
}
