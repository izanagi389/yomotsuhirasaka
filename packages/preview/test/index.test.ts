import { describe, expect, it } from "vitest";

import { READY, REFRESH, acceptPreviewMessage } from "../src/index.ts";

const parent = {};
const expected = { origins: ["https://cms.example.com"], source: parent };
const msg = (origin: string, source: unknown, data: unknown) => ({ origin, source, data });

describe("acceptPreviewMessage", () => {
  it("設定したオリジンの、期待した窓からの決まった形のメッセージだけを受け付ける", () => {
    expect(acceptPreviewMessage(msg("https://cms.example.com", parent, { type: REFRESH }), expected)).toEqual({ type: REFRESH });
    expect(acceptPreviewMessage(msg("https://cms.example.com", parent, { type: READY, extra: 1 }), expected)).toEqual({ type: READY });
  });

  it("オリジンが違えば受け付けない", () => {
    for (const origin of ["https://evil.example.com", "https://cms.example.com.evil.test", "http://cms.example.com", "https://cms.example.com:444", "null", ""]) {
      expect(acceptPreviewMessage(msg(origin, parent, { type: REFRESH }), expected), origin).toBeNull();
    }
  });

  it("送り元の窓が違う・期待した窓がないときは受け付けない", () => {
    expect(acceptPreviewMessage(msg("https://cms.example.com", {}, { type: REFRESH }), expected)).toBeNull();
    expect(acceptPreviewMessage(msg("https://cms.example.com", null, { type: REFRESH }), { origins: expected.origins, source: null })).toBeNull();
  });

  it("知らない形のメッセージは受け付けない", () => {
    for (const data of [null, "cms:preview:refresh", { type: "other" }, [REFRESH]]) {
      expect(acceptPreviewMessage(msg("https://cms.example.com", parent, data), expected)).toBeNull();
    }
  });
});
