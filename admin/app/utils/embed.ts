/** 埋め込みのURLが、許可した提供元の https のURLか（サーバーの検査と同じ条件の目安。最終的な判断はサーバー）。 */
export function embedAllowed(src: string, providers: string[]): boolean {
  try {
    const u = new URL(src);
    return u.protocol === "https:" && !u.username && !u.password && !u.port && providers.includes(u.hostname);
  } catch {
    return false;
  }
}
