// テンプレートの関数（と検索の対象の指定）の、サーバーと同じ規則による早めの検査（13-security.md「デプロイ」: 判定はサーバーが行い、ここは早く知らせるだけ）。
// サーバーの検査は server/src/schema/manifest.rs（INV-NODE-01、INV-HOOK-01）。

type Json = Record<string, unknown>;
const isObject = (v: unknown): v is Json => typeof v === "object" && v !== null && !Array.isArray(v);
const list = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);

const SYNC_HOOKS = ["beforeSave", "beforePublish"];
const ASYNC_HOOKS = ["afterSave", "afterPublish", "afterUnpublish"];

/** 関数の宣言の検査。kind は、外部への送信を許すか（非同期のフック）と、ホスト関数を使えるか（変換関数は使えない）。 */
function checkFunction(path: string, f: unknown, kind: "async" | "sync" | "conversion", problems: string[]): void {
  if (!isObject(f)) return;
  const permissions = list(f["permissions"]);
  const hosts = list(f["allowedHosts"]);
  if (kind === "conversion") {
    if (permissions.length > 0 || hosts.length > 0) problems.push(`${path}.permissions: conversion functions are pure and cannot use host functions`);
    return;
  }
  const fetches = permissions.includes("http.fetch");
  if (fetches && kind !== "async") {
    problems.push(`${path}.permissions: "http.fetch" is only allowed in asynchronous hooks (${ASYNC_HOOKS.join(", ")})`);
  }
  if (fetches && hosts.length === 0) problems.push(`${path}.allowedHosts: list the hosts that "http.fetch" may send to`);
  if (!fetches && hosts.length > 0) problems.push(`${path}.allowedHosts: only for functions with the "http.fetch" permission`);
}

/** template.json の関数の宣言を検査し、問題の一覧を返す（空なら問題なし）。 */
export function checkTemplateFunctions(template: Json): string[] {
  const problems: string[] = [];
  const body = isObject(template["body"]) ? template["body"] : {};
  for (const [i, node] of list(body["customNodes"]).entries()) {
    if (!isObject(node)) continue;
    const path = `body.customNodes[${i}]`;
    if (node["toMarkdown"] === undefined || node["toMarkdown"] === null) {
      problems.push(`${path}.toMarkdown: a Markdown conversion function is required for every custom node (create one with: cms node scaffold ${String(node["name"] ?? "<name>")})`);
    }
    checkFunction(`${path}.toMarkdown`, node["toMarkdown"], "conversion", problems);
    checkFunction(`${path}.toHtml`, node["toHtml"], "conversion", problems);
  }
  for (const [i, ct] of list(template["contentTypes"]).entries()) {
    if (!isObject(ct)) continue;
    // 検索・質問応答の対象にできるのは text と richtext だけ（server/src/schema/manifest.rs と同じ規則）
    for (const [j, f] of list(ct["fields"]).entries()) {
      if (isObject(f) && f["searchable"] === true && f["type"] !== "text" && f["type"] !== "richtext") {
        problems.push(`contentTypes[${i}].fields[${j}].searchable: only text and richtext fields can be searchable`);
      }
    }
    checkFunction(`contentTypes[${i}].urlFunction`, ct["urlFunction"], "sync", problems);
    const hooks = isObject(ct["hooks"]) ? ct["hooks"] : {};
    for (const k of SYNC_HOOKS) checkFunction(`contentTypes[${i}].hooks.${k}`, hooks[k], "sync", problems);
    for (const k of ASYNC_HOOKS) checkFunction(`contentTypes[${i}].hooks.${k}`, hooks[k], "async", problems);
  }
  return problems;
}
