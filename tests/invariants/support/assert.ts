// 約束のテストで共通に使う確認。

import { expect } from "vitest";
import { isRejected, isSuccess, REJECTED_STATUSES, type HttpResponse } from "./http.js";

/** 拒否されたこと（401・403・404のどれか）を確かめる。 */
export function expectRejected(res: HttpResponse, what: string): void {
  expect(isRejected(res), `${what} は拒否されるべきなのに、状態コード ${res.status} が返った（期待: ${REJECTED_STATUSES.join("・")}）`).toBe(true);
}

/** 成功したこと（2xx）を確かめる。拒否の確認が「何でも拒否する」実装で通ってしまわないための対照。 */
export function expectSuccess(res: HttpResponse, what: string): void {
  expect(isSuccess(res), `${what} は成功するべきなのに、状態コード ${res.status} が返った`).toBe(true);
}

/** 応答のどこにも（本文・ヘッダー）、指定した値が含まれないことを確かめる。 */
export function expectNotLeaked(res: HttpResponse, secret: string, what: string): void {
  const headers = [...res.headers.entries()].map(([k, v]) => `${k}: ${v}`).join("\n");
  expect(res.text.includes(secret), `${what} の応答の本文に、含まれてはいけない値が含まれている`).toBe(false);
  expect(headers.includes(secret), `${what} の応答のヘッダーに、含まれてはいけない値が含まれている`).toBe(false);
}

/** 受け付けられなかったこと（4xx）を確かめる。認証・認可以外の理由（検証、未登録のクエリなど）の拒否に使う。 */
export function expectRefused(res: HttpResponse, what: string): void {
  expect(res.status >= 400 && res.status < 500, `${what} は受け付けられないべきなのに、状態コード ${res.status} が返った（期待: 4xx）`).toBe(true);
}

/** 回数制限で止められたこと（429）を確かめる。 */
export function expectRateLimited(res: HttpResponse, what: string): void {
  expect(res.status, `${what} は回数制限で止められるべき`).toBe(429);
}

/** 文字列のどこにも、指定した値が含まれないことを確かめる（メール・ログ・Webhookなど、HTTPの応答以外に使う）。 */
export function expectTextNotLeaked(text: string, secret: string, what: string): void {
  expect(text.includes(secret), `${what} に、含まれてはいけない値が含まれている`).toBe(false);
}
