// サイトのサーバーが持つ、CMSから取ったもののキャッシュ（Q145）。CMSのWebhookを受けたら消す（Q146）。
// サイトのサーバーが複数のプロセスで動くときは、Webhookを受けたプロセスだけが消える。ほかのプロセスは、保持する時間（ttl）の後に取り直す。

/** 保持する時間（ミリ秒）。Webhookが届かなかったときの取り直しの間隔でもある。 */
export const TTL_MS = 5 * 60 * 1000;

const entries = new Map<string, { value: unknown; at: number }>();

/** キャッシュにあればそれを、なければ load の結果を入れて返す。load が失敗したら、古い値があればそれを返す。 */
export async function cached<T>(key: string, load: () => Promise<T>, now: number = Date.now()): Promise<T> {
  const hit = entries.get(key);
  if (hit && now - hit.at < TTL_MS) return hit.value as T;
  try {
    const value = await load();
    entries.set(key, { value, at: now });
    return value;
  } catch (e) {
    if (hit) return hit.value as T;
    throw e;
  }
}

/** すべて消す（CMSの記事の状態が変わったとき）。 */
export function clearAll(): void {
  entries.clear();
}
