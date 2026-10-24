// 「やること」の一覧（GET /workspaces/{ws}/todo）。ナビの件数と「やること」画面で同じデータを共有する。
import type { TodoItem } from "~/utils/content";

export function useTodo(workspaceId: string) {
  const api = useApi();
  return useAsyncData(`todo:${workspaceId}`, () => api.get<{ items: TodoItem[] }>(`/workspaces/${workspaceId}/todo`).then((r) => r.items));
}
