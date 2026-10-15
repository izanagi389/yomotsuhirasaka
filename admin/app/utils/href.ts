/**
 * リンク先として使えるか（サーバーの body.rs の valid_href と同じ条件の目安。最終的な判断はサーバー）。
 * https・http・mailto・tel と、相対パス・`#` のアンカー・`?` のクエリだけ。
 */
export function hrefAllowed(href: string): boolean {
  if (!href || href.length > 2000 || /[\s\p{Cc}]/u.test(href)) return false;
  if (/^(#|\?|\.\/|\.\.\/)/.test(href)) return true;
  if (href.startsWith("/")) return !href.startsWith("//") && !href.startsWith("/\\");
  const scheme = /^([a-zA-Z][a-zA-Z0-9+.-]*):/.exec(href)?.[1]?.toLowerCase();
  return scheme !== undefined && ["https", "http", "mailto", "tel"].includes(scheme);
}
