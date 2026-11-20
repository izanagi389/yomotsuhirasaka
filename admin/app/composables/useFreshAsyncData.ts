// 複数の画面や常に表示されている部品（サイドバー）が同じキーで共有するデータ。
// Nuxt の useAsyncData は、同じキーのデータを誰かが使っている間はキャッシュを捨てず、取得済みなら取り直さない。
// 画面の切り替えでは新しい画面の準備が古い画面の片付けより先に走るので、共有のキーは画面を移っても古いまま残る
// （例: 記事を削除して一覧に戻っても、削除した記事が一覧に残る）。
// ここでは、取得済みのデータが残っているときも、画面を開くたびに取り直す。取り直しを待ってから画面を出すので、古い内容は見えない。
export function useFreshAsyncData<T>(key: string, handler: () => Promise<T>) {
  const nuxtApp = useNuxtApp();
  const cached = nuxtApp._asyncData[key]?.status.value === "success";
  const result = useAsyncData(key, handler);
  if (!cached) return result;
  return Object.assign(
    Promise.all([result, result.refresh()]).then(([r]) => r),
    result,
  ) as typeof result;
}
