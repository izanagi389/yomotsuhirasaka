// モジュールが自分で登録するクエリの名前（サイトの cms/queries の名前とぶつからないよう、モジュールが確かめる）。
export const REDIRECTS_QUERY = "cmsRedirects";
export const REDIRECTS_QUERY_TEXT = "{ redirects { from to status } }";
export const CORRECTIONS_QUERY = "cmsCorrections";
export const CORRECTIONS_QUERY_TEXT =
  "query ($locale: String!, $limit: Int, $offset: Int) { corrections(locale: $locale, limit: $limit, offset: $offset) { entryId contentType url title correctedAt description sections } }";
/** サイトの cms/queries で使えない名前。 */
export const RESERVED_QUERIES = [REDIRECTS_QUERY, CORRECTIONS_QUERY];
