import { IncomingMessage, ServerResponse } from "node:http";
import { Socket } from "node:net";

import { createEvent, getResponseHeader, setResponseHeader } from "h3";
import { describe, expect, it, vi } from "vitest";

vi.mock("nitropack/runtime", () => ({ defineNitroPlugin: (f: unknown) => f }));

const { markPreviewResponse } = await import("../src/runtime/server/preview-no-store.ts");

function eventWith(cookie?: string) {
  const req = new IncomingMessage(new Socket());
  req.method = "GET";
  req.url = "/ja/blog/a";
  if (cookie) req.headers["cookie"] = cookie;
  return createEvent(req, new ServerResponse(req));
}

describe("markPreviewResponse", () => {
  it("プレビューのクッキーがあれば、ページの応答も含めて共有のキャッシュに入れさせない", () => {
    const event = eventWith("yomi_preview=yomi_prv_abc");
    // ページ側のキャッシュの設定より後に付ける
    setResponseHeader(event, "Cache-Control", "public, s-maxage=600");
    markPreviewResponse(event);
    expect(getResponseHeader(event, "Cache-Control")).toBe("private, no-store");
  });

  it("クッキーがない・形が違えば、何も変えない", () => {
    for (const cookie of [undefined, "yomi_preview=not-a-token", "other=yomi_prv_abc"]) {
      const event = eventWith(cookie);
      setResponseHeader(event, "Cache-Control", "public, s-maxage=600");
      markPreviewResponse(event);
      expect(getResponseHeader(event, "Cache-Control")).toBe("public, s-maxage=600");
    }
  });
});
