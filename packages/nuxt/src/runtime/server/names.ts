// モジュールが自分で登録するクエリの名前（サイトの yomi/queries の名前とぶつからないよう、モジュールが確かめる）。
export const REDIRECTS_QUERY = "yomiRedirects";
export const REDIRECTS_QUERY_TEXT = "{ redirects { from to status } }";
export const CORRECTIONS_QUERY = "yomiCorrections";
export const CORRECTIONS_QUERY_TEXT =
  "query ($locale: String!, $limit: Int, $offset: Int) { corrections(locale: $locale, limit: $limit, offset: $offset) { entryId contentType url title correctedAt description sections } }";
export const AI_CATALOG_QUERY = "yomiAiCatalog";
export const AI_CATALOG_QUERY_TEXT =
  "query ($locale: String!, $limit: Int, $offset: Int) { aiCatalog(locale: $locale, limit: $limit, offset: $offset) { entryId contentType url title locale updatedAt } }";
export const AI_MARKDOWN_QUERY = "yomiAiMarkdown";
export const AI_MARKDOWN_QUERY_TEXT = "query ($url: String!) { aiMarkdownByUrl(url: $url) }";
export const AI_OPT_OUTS_QUERY = "yomiAiOptOuts";
export const AI_OPT_OUTS_QUERY_TEXT = "{ aiTrainingOptOuts }";
export const SEARCH_QUERY = "yomiSearch";
export const SEARCH_QUERY_TEXT =
  "query ($text: String!, $locale: String!, $mode: YomiSearchMode, $first: Int) { search(text: $text, locale: $locale, mode: $mode, first: $first) { vectorSearch keywordSearch hits { entryId locale url anchor breadcrumb excerpt matches { start length } score } } }";
export const ANSWER_QUERY = "yomiAnswer";
/** 回答のストリーミング。回答キャッシュから返したか（cached）は選ばない（ブラウザに渡さないため）。 */
export const ANSWER_QUERY_TEXT =
  "subscription ($question: String!, $locale: String!, $visitorId: String) { answerStream(question: $question, locale: $locale, visitorId: $visitorId) { __typename ... on YomiAnswerVerdict { answerable sources { citation entryId locale url anchor breadcrumb versionId lastReviewedAt reviewExpired } } ... on YomiAnswerDelta { text } ... on YomiAnswerFinal { status text truncated sources { citation entryId locale url anchor breadcrumb versionId lastReviewedAt reviewExpired } } } }";
/** サイトの yomi/queries で使えない名前。 */
export const RESERVED_QUERIES = [REDIRECTS_QUERY, CORRECTIONS_QUERY, AI_CATALOG_QUERY, AI_MARKDOWN_QUERY, AI_OPT_OUTS_QUERY, SEARCH_QUERY, ANSWER_QUERY];

/**
 * 汎用のルートから呼べないクエリか。訂正の一覧（公開の情報で、`useYomiCorrections` が汎用のルートを使う）のほかは、専用のルートだけから呼ぶ
 * （検索と回答は、訪問者ごとの回数の制限とキャッシュを専用のルートで行うので、汎用のルートから呼ばせると迂回できる）。
 */
export function isModuleOnlyQuery(name: string): boolean {
  return name !== CORRECTIONS_QUERY && (RESERVED_QUERIES as readonly string[]).includes(name);
}
