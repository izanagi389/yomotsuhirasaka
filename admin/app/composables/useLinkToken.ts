// リセット・招待のリンクのトークン（URLの # の後ろ。サーバーやログに送られないよう、クエリではなくフラグメントに置かれる）。
// 読んだら、アドレスバーからは消す。
export function useLinkToken() {
  const token = ref<string | null>(null);
  onMounted(() => {
    const m = /(?:^#|&)token=([A-Za-z0-9_-]+)/.exec(window.location.hash);
    token.value = m?.[1] ?? null;
    if (window.location.hash) history.replaceState(history.state, "", window.location.pathname + window.location.search);
  });
  return token;
}
