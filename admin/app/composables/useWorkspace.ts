// ワークスペースの情報（自分のロール）と、適用済みのスキーマ。記事の画面が使う。
import type { SchemaInfo, WorkspaceInfo } from "~/utils/content";

export async function useWorkspace(id: string) {
  const api = useApi();
  const { data, error, refresh } = await useAsyncData(`workspace:${id}`, async () => {
    const [workspace, schema] = await Promise.all([
      api.get<WorkspaceInfo>(`/workspaces/${id}`),
      api.get<SchemaInfo>(`/workspaces/${id}/schema`),
    ]);
    return { workspace, schema };
  });
  return {
    workspace: computed(() => data.value?.workspace ?? null),
    schema: computed(() => data.value?.schema ?? null),
    manifest: computed(() => data.value?.schema.manifest ?? null),
    error,
    refresh,
  };
}
