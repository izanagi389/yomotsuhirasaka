// 自分が使えるワークスペースの一覧（サーバー管理者はすべて）。一覧の画面・サーバーの管理・サイドバーの切り替えで共有する。

export interface WorkspaceSummary {
  id: string;
  name: string;
  template: string;
  role: string | null;
  schemaState: "none" | "ok" | "pending";
  crest?: import("~/utils/kamon").Crest | null;
}

export function useWorkspaces() {
  const api = useApi();
  return useAsyncData("workspaces", () => api.get<WorkspaceSummary[]>("/workspaces"));
}
