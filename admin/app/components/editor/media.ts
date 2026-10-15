// 画像のノードビューが使う、メディアの情報の読み込み（同じメディアは1回だけ読む）。
import type { MediaInfo } from "~/utils/content";

const cache = new Map<string, Promise<MediaInfo | null>>();

export function mediaInfo(workspaceId: string, mediaId: string): Promise<MediaInfo | null> {
  const key = `${workspaceId}/${mediaId}`;
  let p = cache.get(key);
  if (!p) {
    p = useApi()
      .get<MediaInfo>(`/workspaces/${workspaceId}/media/${mediaId}`)
      .catch(() => {
        cache.delete(key);
        return null;
      });
    cache.set(key, p);
  }
  return p;
}
