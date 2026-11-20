// スキーマの名前を、管理画面に出す言葉にする（02-schema-and-delivery-api.md「表示名」）。
// フィールドとコンテンツタイプは、管理画面の言語の label があればそれを、なければ内部名を出す（別の言語の label では代えない）。
// ロケールは、ブラウザの言語名にして「日本語（ja）」の形にする。
import type { ContentType, Field, Label } from "~/utils/content";

function pick(label: Label | undefined, uiLocale: string): string | undefined {
  return uiLocale === "ja" ? label?.ja : uiLocale === "en" ? label?.en : undefined;
}

/** フィールドの表示名。 */
export function fieldLabel(field: Pick<Field, "name" | "label">, uiLocale: string): string {
  return pick(field.label, uiLocale) ?? field.name;
}

/** コンテンツタイプの表示名。マニフェストに見つからなければ内部名。 */
export function typeLabel(type: Pick<ContentType, "name" | "label"> | string | undefined, uiLocale: string, manifest?: { contentTypes: ContentType[] } | null): string {
  if (typeof type === "string") {
    const found = manifest?.contentTypes.find((c) => c.name === type);
    return found ? typeLabel(found, uiLocale) : type;
  }
  if (!type) return "";
  return pick(type.label, uiLocale) ?? type.name;
}

/** コンテンツのロケールの表示（例: 「日本語（ja）」「Japanese (ja)」）。言語名が分からなければコードだけ。 */
export function localeName(code: string, uiLocale: string): string {
  let name: string | undefined;
  try {
    name = new Intl.DisplayNames([uiLocale], { type: "language" }).of(code);
  } catch {
    name = undefined;
  }
  if (!name || name === code) return code;
  return uiLocale === "ja" ? `${name}（${code}）` : `${name} (${code})`;
}
