// 新しい型のフィールドの値と、ブラウザの入力欄の値の行き来（server/src/content/fields.rs、datetime.rs）。
// 日時は UTC で保存・配信し、管理画面ではブラウザのタイムゾーンで見せる（02-schema-and-delivery-api.md「フィールドの型」）。

const pad = (n: number) => String(n).padStart(2, "0");

/** 保存した日時（UTC の ISO 8601）を、`datetime-local` の入力欄の値（ブラウザの時刻、秒まで）にする。 */
export function toLocalInput(value: unknown): string {
  if (typeof value !== "string") return "";
  const d = new Date(value);
  if (Number.isNaN(d.getTime())) return "";
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

/** `datetime-local` の入力欄の値を、保存する日時（UTC、秒まで）にする。空や読めない値は null。 */
export function fromLocalInput(input: string): string | null {
  if (input === "") return null;
  const d = new Date(input);
  if (Number.isNaN(d.getTime())) return null;
  // サーバーは小数の秒を受け付けないので、ミリ秒を落とす
  return d.toISOString().replace(/\.\d{3}Z$/, "Z");
}

/** 数値の入力欄の値を、保存する数値にする。空や読めない値は null。 */
export function toNumber(input: string | number): number | null {
  if (input === "") return null;
  const n = Number(input);
  return Number.isFinite(n) ? n : null;
}

/** ブラウザのタイムゾーンの名前（日時の入力欄の注記に使う）。 */
export function browserTimeZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone;
}

/** グループの新しい件の固定のID。サーバーの形（`i-` と英小文字・数字10文字。groups.rs）にそろえる。 */
export function newItemId(): string {
  const alphabet = "0123456789abcdefghijklmnopqrstuvwxyz";
  const bytes = new Uint8Array(10);
  crypto.getRandomValues(bytes);
  return "i-" + Array.from(bytes, (b) => alphabet[b % alphabet.length]).join("");
}
