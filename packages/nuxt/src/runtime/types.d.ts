declare module "#yomi/queries" {
  const queries: Record<string, string>;
  export default queries;
}

declare module "#yomi/protected-paths" {
  /** CMS のリダイレクトを当てない、サイト自身のパス（ビルドのときに分かる固定のページと、モジュールのルート）。 */
  const protection: import("./server/redirect-rules.ts").Protection;
  export default protection;
}
