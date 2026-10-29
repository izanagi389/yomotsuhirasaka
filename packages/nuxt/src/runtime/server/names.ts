// モジュールが自分で登録するクエリの名前（サイトの cms/queries の名前とぶつからないよう、モジュールが確かめる）。
export const REDIRECTS_QUERY = "cmsRedirects";
export const REDIRECTS_QUERY_TEXT = "{ redirects { from to status } }";
export const CORRECTIONS_QUERY = "cmsCorrections";
export const CORRECTIONS_QUERY_TEXT =
  "query ($locale: String!, $limit: Int, $offset: Int) { corrections(locale: $locale, limit: $limit, offset: $offset) { entryId contentType url title correctedAt description sections } }";
export const AI_CATALOG_QUERY = "cmsAiCatalog";
export const AI_CATALOG_QUERY_TEXT =
  "query ($locale: String!, $limit: Int, $offset: Int) { aiCatalog(locale: $locale, limit: $limit, offset: $offset) { entryId contentType url title locale updatedAt } }";
export const AI_MARKDOWN_QUERY = "cmsAiMarkdown";
export const AI_MARKDOWN_QUERY_TEXT = "query ($url: String!) { aiMarkdownByUrl(url: $url) }";
export const AI_OPT_OUTS_QUERY = "cmsAiOptOuts";
export const AI_OPT_OUTS_QUERY_TEXT = "{ aiTrainingOptOuts }";
export const SEARCH_QUERY = "cmsSearch";
export const SEARCH_QUERY_TEXT =
  "query ($text: String!, $locale: String!, $mode: CmsSearchMode, $first: Int) { search(text: $text, locale: $locale, mode: $mode, first: $first) { vectorSearch keywordSearch hits { entryId locale url anchor breadcrumb excerpt matches { start length } score } } }";
/** サイトの cms/queries で使えない名前。 */
export const RESERVED_QUERIES = [REDIRECTS_QUERY, CORRECTIONS_QUERY, AI_CATALOG_QUERY, AI_MARKDOWN_QUERY, AI_OPT_OUTS_QUERY, SEARCH_QUERY];
