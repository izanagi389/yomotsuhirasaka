/**
 * リンク先として描いてよいか（CMSの本文の検証と同じ条件。CMSは保存時に検証しているが、描く側でも確かめる）。
 * https・http・mailto・tel と、相対パス・`#` のアンカー・`?` のクエリだけ。
 */
export function hrefAllowed(href: unknown): href is string {
  if (typeof href !== "string" || !href || href.length > 2000 || /[\s\p{Cc}]/u.test(href)) return false;
  if (/^(#|\?|\.\/|\.\.\/)/.test(href)) return true;
  if (href.startsWith("/")) return !href.startsWith("//") && !href.startsWith("/\\");
  const scheme = /^([a-zA-Z]+):/.exec(href)?.[1]?.toLowerCase();
  return scheme !== undefined && ["https", "http", "mailto", "tel"].includes(scheme);
}

/** 画像のURLとして使ってよいか（CMSが返す配信のURL。https か、同じサイトの相対パス）。 */
export function srcAllowed(src: unknown): src is string {
  if (typeof src !== "string" || !src || /[\s\p{Cc}]/u.test(src)) return false;
  if (src.startsWith("/")) return !src.startsWith("//");
  return /^https?:\/\//i.test(src);
}
