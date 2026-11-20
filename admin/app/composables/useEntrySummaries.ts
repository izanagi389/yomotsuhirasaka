// 記事の一覧（GET /workspaces/{ws}/entries）。一覧の画面と、編集画面の参照フィールドの候補で同じデータを共有する。
import type { EntrySummary, SchemaInfo } from "~/utils/content";

export function useEntrySummaries(ws: string, schema: Ref<SchemaInfo | null>) {
  const api = useApi();
  return useFreshAsyncData(`entries:${ws}`, () =>
    schema.value?.manifest ? api.get<EntrySummary[]>(`/workspaces/${ws}/entries`) : Promise.resolve([] as EntrySummary[]),
  );
}
