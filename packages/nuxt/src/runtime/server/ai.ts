// AI向けの素材の書き出し（07-ai-delivery.md）: 記事のMarkdown版（`<記事のURL>.md`）、`/llms.txt`、`robots.txt`。
// 素材（Markdown版・目録・学習を拒否した記事のURL）はCMSが作り、ここは形式に合わせて並べるだけ。`llms.txt` は提案段階の
// 形式なので、素材と出力を分けておく。
// ここは Nitro に依存しない（テストで直接呼ぶ）。

/** 配信APIの `aiCatalog` の1件。 */
export interface YomiAiCatalogItem {
  entryId: string;
  contentType: string;
  url: string;
  title: string | null;
  locale: string;
  updatedAt: string;
}

/** 学習用のクローラーの既定の一覧（User-agent）。検索・引用のためのクローラー（読んで引用するAI）は含めない（Q33）。 */
export const DEFAULT_AI_TRAINING_CRAWLERS = [
  "GPTBot",
  "Google-Extended",
  "CCBot",
  "ClaudeBot",
  "anthropic-ai",
  "Applebot-Extended",
  "meta-externalagent",
  "Bytespider",
] as const;

/** モジュールが使う形に確かめた設定（runtimeConfig.yomi.ai）。 */
export interface AiConfig {
  /** サイトのオリジン（例: https://example.com）。 */
  siteOrigin: string;
  /** ロケール（順番は llms.txt の並び）→ 名前と説明。 */
  locales: { locale: string; title: string; description: string }[];
  llmsTxt: boolean;
  markdown: boolean;
  /** robots.txt を返すとき、学習用のクローラーと、後ろに足す行。 */
  robotsTxt: { crawlers: string[]; extra: string } | null;
}

export class AiConfigError extends Error {}

const LOCALE = /^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$/;
const USER_AGENT = /^[A-Za-z0-9._-]{1,64}$/;

export interface AiOptions {
  locales?: Record<string, { title?: unknown; description?: unknown } | undefined>;
  llmsTxt?: boolean;
  markdown?: boolean;
  robotsTxt?: false | { addCrawlers?: unknown; removeCrawlers?: unknown; extra?: unknown };
}

/** nuxt.config の `yomi.ai` を確かめて、使う形にする（ビルドのときに止める）。 */
export function checkAiConfig(raw: AiOptions, siteUrl: string): AiConfig {
  if (!siteUrl) throw new AiConfigError("yomi.ai: set yomi.siteUrl (the site's URL) to publish AI materials");
  const locales: AiConfig["locales"] = [];
  for (const [locale, v] of Object.entries(raw.locales ?? {})) {
    if (!LOCALE.test(locale)) throw new AiConfigError(`yomi.ai.locales.${locale}: the key must be a locale (e.g. ja, en-US)`);
    if (typeof v?.title !== "string" || !v.title.trim()) throw new AiConfigError(`yomi.ai.locales.${locale}.title: required`);
    if (v.description !== undefined && typeof v.description !== "string") throw new AiConfigError(`yomi.ai.locales.${locale}.description: must be a string`);
    locales.push({ locale, title: v.title, description: (v.description as string | undefined) ?? "" });
  }
  if (locales.length === 0) throw new AiConfigError("yomi.ai.locales: list at least one locale");
  const crawlerList = (v: unknown, key: string): string[] => {
    if (v === undefined) return [];
    if (!Array.isArray(v) || !v.every((c) => typeof c === "string" && USER_AGENT.test(c))) {
      throw new AiConfigError(`yomi.ai.robotsTxt.${key}: must be a list of user agents (letters, digits, . _ -)`);
    }
    return v as string[];
  };
  let robotsTxt: AiConfig["robotsTxt"] = null;
  if (raw.robotsTxt) {
    const remove = new Set(crawlerList(raw.robotsTxt.removeCrawlers, "removeCrawlers").map((c) => c.toLowerCase()));
    const crawlers = [...new Set([...DEFAULT_AI_TRAINING_CRAWLERS, ...crawlerList(raw.robotsTxt.addCrawlers, "addCrawlers")])].filter(
      (c) => !remove.has(c.toLowerCase()),
    );
    const extra = raw.robotsTxt.extra ?? "";
    if (typeof extra !== "string") throw new AiConfigError("yomi.ai.robotsTxt.extra: must be a string");
    robotsTxt = { crawlers, extra };
  }
  return { siteOrigin: new URL(siteUrl).origin, locales, llmsTxt: raw.llmsTxt ?? true, markdown: raw.markdown ?? true, robotsTxt };
}

/** 記事のURLのパス（末尾の `/` を除く）。サイトと別のオリジンや、サイトのトップのURLなら null。 */
export function articlePath(url: string, siteOrigin: string): string | null {
  let u: URL;
  try {
    u = new URL(url);
  } catch {
    return null;
  }
  if (u.origin !== siteOrigin || u.search || u.hash) return null;
  const path = u.pathname.replace(/\/+$/, "");
  return path && path !== "/" ? path : null;
}

/** 記事のMarkdown版のパス（`<記事のURLのパス>.md`）。 */
export function markdownPath(url: string, siteOrigin: string): string | null {
  const path = articlePath(url, siteOrigin);
  return path ? `${path}.md` : null;
}

/** Markdownのリンクの文字として書けるようにする。 */
function linkText(s: string): string {
  return s.replace(/[\r\n]+/g, " ").replace(/[\\[\]`*_<>]/g, "\\$&");
}

/** Markdownのリンク先として書けるようにする（括弧と空白を符号化する）。 */
function linkTarget(s: string): string {
  return s.replace(/[()\s<>]/g, (c) => `%${c.charCodeAt(0).toString(16).toUpperCase().padStart(2, "0")}`);
}

/** `/llms.txt`（https://llmstxt.org の形）。記事は、Markdown版へのリンクとして並べる。 */
export function toLlmsTxt(config: AiConfig, catalogs: Record<string, YomiAiCatalogItem[]>): string {
  const [first] = config.locales;
  const lines = [`# ${linkText(first!.title)}`, ""];
  if (first!.description) lines.push(`> ${first!.description.replace(/[\r\n]+/g, " ")}`, "");
  for (const { locale, title, description } of config.locales) {
    lines.push(`## ${linkText(title)}`, "");
    if (description && locale !== first!.locale) lines.push(description.replace(/[\r\n]+/g, " "), "");
    for (const item of catalogs[locale] ?? []) {
      const md = markdownPath(item.url, config.siteOrigin);
      if (!md) continue;
      lines.push(`- [${linkText(item.title ?? item.url)}](${linkTarget(config.siteOrigin + md)})`);
    }
    lines.push("");
  }
  return lines.join("\n");
}

/** robots.txt のパス（`*` と `$` は意味を持つので符号化する）。 */
function robotsPath(url: string, siteOrigin: string): string | null {
  let u: URL;
  try {
    u = new URL(url);
  } catch {
    return null;
  }
  if (u.origin !== siteOrigin) return null;
  return (u.pathname + u.search).replace(/\*/g, "%2A").replace(/\$/g, "%24");
}

/**
 * robots.txt。学習用のクローラーに、学習への利用を拒否した記事（とそのMarkdown版）を Disallow で並べる。
 * 意思表示であって、技術的に止めるものではない（07-ai-delivery.md）。サイトの行（extra）は後ろに足す。
 */
export function toRobotsTxt(config: AiConfig, optOutUrls: string[]): string {
  const robots = config.robotsTxt;
  if (!robots) return "";
  const paths = new Set<string>();
  for (const url of optOutUrls) {
    const p = robotsPath(url, config.siteOrigin);
    if (!p) continue;
    paths.add(p);
    const md = markdownPath(url, config.siteOrigin);
    if (md) paths.add(md.replace(/\*/g, "%2A").replace(/\$/g, "%24"));
  }
  const lines = ["# Articles that opt out of use for AI training (generated by @yomotsuhirasaka/nuxt)"];
  if (robots.crawlers.length > 0) {
    for (const c of robots.crawlers) lines.push(`User-agent: ${c}`);
    if (paths.size === 0) lines.push("Disallow:");
    for (const p of [...paths].sort()) lines.push(`Disallow: ${p}`);
  }
  lines.push("");
  if (robots.extra.trim()) lines.push(robots.extra.trim(), "");
  return lines.join("\n");
}
