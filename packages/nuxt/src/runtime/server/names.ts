// モジュールが自分で登録するクエリの名前（サイトの cms/queries の名前とぶつからないよう、モジュールが確かめる）。
export const REDIRECTS_QUERY = "cmsRedirects";
export const REDIRECTS_QUERY_TEXT = "{ redirects { from to status } }";
