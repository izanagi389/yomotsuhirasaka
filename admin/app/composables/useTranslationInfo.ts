// 記事の翻訳の情報（翻訳グループ、元の記事、原文の更新）。記事の画面の知らせ・タブの印・翻訳タブで同じデータを共有する。
import type { TranslationInfo } from "~/utils/content";

export const translationInfoKey = (entryId: string) => `translation:${entryId}`;

export function useTranslationInfo(workspaceId: string, entryId: string) {
  const api = useApi();
  return useAsyncData(translationInfoKey(entryId), () => api.get<TranslationInfo>(`/workspaces/${workspaceId}/entries/${entryId}/translation`));
}
