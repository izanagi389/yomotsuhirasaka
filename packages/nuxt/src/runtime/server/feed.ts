// 訂正フィード（RSS 2.0 と JSON Feed 1.1）を組み立てる（06-corrections-and-freshness.md「訂正フィード」、Q48）。
// 載せるのは配信APIの `corrections` が返すもの（「訂正として公開」したものだけ。INV-COR-03）。変更前の本文は配信APIも返さない。
// ここは Nitro に依存しない（テストで直接呼ぶ）。

/** 配信APIの `corrections` の1件。 */
export interface CmsCorrectionItem {
  entryId: string;
  contentType: string;
  url: string;
  title: string | null;
  correctedAt: string;
  description: string | null;
  sections: string[];
}

/** 1つのロケールのフィードの設定（サイトの nuxt.config の `cms.corrections.feeds.<ロケール>`）。 */
export interface CorrectionFeedConfig {
  locale: string;
  /** フィードのパス（拡張子なし）。`<path>.xml` が RSS、`<path>.json` が JSON Feed。 */
  path: string;
  /** フィードの名前。 */
  title: string;
  /** サイトのURL（フィードの `link` と `home_page_url`）。 */
  link: string;
  /** フィードの説明（なければ title）。 */
  description: string;
  /** 載せる件数（1〜100）。 */
  limit: number;
}

/** 配信APIが一度に返す件数の上限。 */
export const MAX_FEED_ITEMS = 100;
export const DEFAULT_FEED_ITEMS = 50;

export class FeedConfigError extends Error {}

const PATH = /^(\/[A-Za-z0-9._~-]+)+$/;
const LOCALE = /^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$/;

/** nuxt.config のフィードの設定を確かめて、使う形にする（ビルドのときに止める）。 */
export function checkFeedConfig(
  locale: string,
  raw: { path?: unknown; title?: unknown; link?: unknown; description?: unknown; limit?: unknown },
): CorrectionFeedConfig {
  const where = `cms.corrections.feeds.${locale}`;
  if (!LOCALE.test(locale)) throw new FeedConfigError(`${where}: the key must be a locale (e.g. ja, en-US)`);
  const path = raw.path;
  if (typeof path !== "string" || !PATH.test(path) || /\/\.\.?(\/|$)/.test(path) || /\.(xml|json)$/i.test(path)) {
    throw new FeedConfigError(`${where}.path: must start with / and have no extension (e.g. /corrections)`);
  }
  const title = raw.title;
  if (typeof title !== "string" || !title.trim()) throw new FeedConfigError(`${where}.title: required`);
  if (typeof raw.link !== "string" || !/^https?:\/\//i.test(raw.link)) throw new FeedConfigError(`${where}.link: must be the site's http(s) URL`);
  const link = new URL(raw.link).toString();
  const description = raw.description === undefined ? title : raw.description;
  if (typeof description !== "string") throw new FeedConfigError(`${where}.description: must be a string`);
  const limit = raw.limit === undefined ? DEFAULT_FEED_ITEMS : raw.limit;
  if (typeof limit !== "number" || !Number.isInteger(limit) || limit < 1 || limit > MAX_FEED_ITEMS) {
    throw new FeedConfigError(`${where}.limit: must be an integer from 1 to ${MAX_FEED_ITEMS}`);
  }
  return { locale, path, title, link, description, limit };
}

/** XML 1.0 で使えない文字（タブ・改行以外の制御文字、対のないサロゲート、U+FFFE/U+FFFF）を除く。 */
function xmlChars(s: string): string {
  return s.replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F￾￿]|[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g, "");
}

/** XML の文字と属性の値として書けるようにする。 */
export function escapeXml(s: string): string {
  return xmlChars(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&apos;" })[c]!);
}

/** フィードに載せてよい項目か（リンクは http(s) の絶対URL、日時は読めるもの）。CMSが作る値だが、書き出す側でも確かめる。 */
function usable(item: CmsCorrectionItem): boolean {
  return /^https?:\/\/[^\s]+$/i.test(item.url) && !Number.isNaN(Date.parse(item.correctedAt));
}

/** 項目のID。同じ記事でも訂正ごとに別のもの（記事のIDと訂正の日時）。 */
const itemId = (item: CmsCorrectionItem) => `${item.entryId}@${item.correctedAt}`;

/** RSS 2.0。 */
export function toRss(feed: CorrectionFeedConfig, items: CmsCorrectionItem[]): string {
  const list = items.filter(usable);
  const self = new URL(`${feed.path}.xml`, feed.link).toString();
  const lines = [
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom">',
    "<channel>",
    `<title>${escapeXml(feed.title)}</title>`,
    `<link>${escapeXml(feed.link)}</link>`,
    `<description>${escapeXml(feed.description)}</description>`,
    `<language>${escapeXml(feed.locale)}</language>`,
    `<atom:link href="${escapeXml(self)}" rel="self" type="application/rss+xml"/>`,
  ];
  if (list[0]) lines.push(`<lastBuildDate>${new Date(list[0].correctedAt).toUTCString()}</lastBuildDate>`);
  for (const item of list) {
    lines.push(
      "<item>",
      `<title>${escapeXml(item.title ?? item.url)}</title>`,
      `<link>${escapeXml(item.url)}</link>`,
      `<guid isPermaLink="false">${escapeXml(itemId(item))}</guid>`,
      `<pubDate>${new Date(item.correctedAt).toUTCString()}</pubDate>`,
    );
    if (item.description) lines.push(`<description>${escapeXml(item.description)}</description>`);
    lines.push("</item>");
  }
  lines.push("</channel>", "</rss>", "");
  return lines.join("\n");
}

/** JSON Feed 1.1。影響したセクション（アンカーID）は拡張 `_corrections` に入れる。 */
export function toJsonFeed(feed: CorrectionFeedConfig, items: CmsCorrectionItem[]): string {
  return JSON.stringify({
    version: "https://jsonfeed.org/version/1.1",
    title: feed.title,
    home_page_url: feed.link,
    feed_url: new URL(`${feed.path}.json`, feed.link).toString(),
    description: feed.description,
    language: feed.locale,
    items: items.filter(usable).map((item) => ({
      id: itemId(item),
      url: item.url,
      title: item.title ?? item.url,
      content_text: item.description ?? "",
      date_published: new Date(item.correctedAt).toISOString(),
      _corrections: { entry_id: item.entryId, content_type: item.contentType, sections: item.sections },
    })),
  });
}
