//! 配信API（GraphQL）の結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use std::io::Cursor;

use common::content::*;
use common::server::*;
use common::with_scratch;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use serde_json::{Value, json};
use uuid::Uuid;

fn parse(res: &Resp) -> Value {
    serde_json::from_str(&res.body).unwrap_or_else(|_| panic!("{}", res.body))
}

fn error(res: &Resp) -> (u16, Option<String>) {
    (res.status, res.json_str("error"))
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn api_key(server: &Server, root: &Login, ws: Uuid, scopes: &[&str]) -> String {
    let res = send(
        server,
        "POST",
        &format!("/api/admin/workspaces/{ws}/api-keys"),
        root,
        Some(&json!({"scopes": scopes}).to_string()),
    )
    .await;
    assert_eq!(res.status, 201, "{}", res.body);
    res.json_str("key").unwrap()
}

async fn register(server: &Server, ws: Uuid, key: &str, query: &str) -> Resp {
    request(
        server,
        "POST",
        &format!("/ws/{ws}/graphql/queries"),
        &[("Authorization", &format!("Bearer {key}"))],
        Some(&json!({"query": query}).to_string()),
    )
    .await
}

async fn run(server: &Server, ws: Uuid, key: &str, id: &str, variables: Value) -> Resp {
    request(
        server,
        "GET",
        &format!(
            "/ws/{ws}/graphql?id={id}&variables={}",
            enc(&variables.to_string())
        ),
        &[("Authorization", &format!("Bearer {key}"))],
        None,
    )
    .await
}

const POST_FIELDS: &str = "id locale url title slug publishedAt firstPublishedAt translationGroupId alternates { locale url } body related { id title }";

#[tokio::test]
async fn delivering_published_entries() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let other_ws = workspace_with_template(&server, &root, "B", "blog").await;
        create_user(&server.pool, "editor@example.test", false).await;
        add_member(&server, ws, "editor@example.test", "editor").await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let entries = format!("/api/admin/workspaces/{ws}/entries");
        let read = api_key(&server, &root, ws, &["read"]).await;
        let reg = api_key(&server, &root, ws, &["register"]).await;
        let other_read = api_key(&server, &root, other_ws, &["read", "register"]).await;

        // 記事: ja を公開し、en の翻訳を作って公開する。下書きの変更は配信されない
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(RgbImage::from_pixel(3, 2, Rgb([1, 2, 3]))).write_to(&mut out, ImageFormat::Png).unwrap();
        let media = parse(&upload(&server, &format!("/api/admin/workspaces/{ws}/media"), &editor, "image/png", &out.into_inner()).await);
        let media_id = media["id"].as_str().unwrap();
        let body = json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "PUBLISHED-BODY"}]},
            {"type": "image", "attrs": {"mediaId": media_id, "alt": "図"}}
        ]});
        let create = |slug: &str, title: &str, body: Value, related: Vec<String>| {
            json!({"contentType": "post", "fields": {"title": title, "slug": slug, "body": body, "related": related}}).to_string()
        };
        let res = send(&server, "POST", &entries, &editor, Some(&create("dep", "依存先", json!({"type": "doc", "content": []}), vec![]))).await;
        let dep = parse(&res);
        let dep_id = dep["id"].as_str().unwrap().to_owned();
        let dep_group = dep["translationGroupId"].as_str().unwrap().to_owned();
        assert_eq!(send(&server, "POST", &format!("{entries}/{dep_id}/publish"), &editor, Some("{\"revision\":1}")).await.status, 200);
        let res = send(&server, "POST", &entries, &editor, Some(&create("hello", "こんにちは", body, vec![dep_group.clone()]))).await;
        let ja = parse(&res)["id"].as_str().unwrap().to_owned();
        assert_eq!(send(&server, "POST", &format!("{entries}/{ja}/publish"), &editor, Some("{\"revision\":1}")).await.status, 200);
        let res = send(&server, "PUT", &format!("{entries}/{ja}/draft"), &editor, Some(&json!({"revision": 1, "fields": {"title": "DRAFT-ONLY"}}).to_string())).await;
        assert_eq!(res.status, 200);
        let res = send(&server, "POST", &format!("{entries}/{ja}/translations"), &editor, Some("{\"locale\":\"en\"}")).await;
        let en = parse(&res)["id"].as_str().unwrap().to_owned();
        send(&server, "PUT", &format!("{entries}/{en}/draft"), &editor, Some(&json!({"revision": 1, "fields": {"title": "Hello", "slug": "hello"}}).to_string())).await;
        assert_eq!(send(&server, "POST", &format!("{entries}/{en}/publish"), &editor, Some("{\"revision\":2}")).await.status, 200);
        // 公開していない記事
        send(&server, "POST", &entries, &editor, Some(&create("never", "NEVER-PUBLISHED", json!({"type": "doc", "content": []}), vec![]))).await;

        // 登録: 登録用のキーだけ（INV-GQL-02）。キーなし・他のワークスペースのキーは拒否
        let by_id = format!("query($id: ID!, $locale: String) {{ post(id: $id, locale: $locale) {{ {POST_FIELDS} }} }}");
        assert_eq!(register(&server, ws, &read, &by_id).await.status, 403);
        assert_eq!(request(&server, "POST", &format!("/ws/{ws}/graphql/queries"), &[], Some(&json!({"query": by_id}).to_string())).await.status, 401);
        assert_eq!(register(&server, ws, &other_read, &by_id).await.status, 404);
        let res = register(&server, ws, &reg, &by_id).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let by_id_ref = res.json_str("id").unwrap();
        assert_eq!(register(&server, ws, &reg, &by_id).await.json_str("id").unwrap(), by_id_ref, "registering again gives the same ID");
        // 登録の検査: スキーマに合わない、複数の操作、ミューテーション、深すぎる・広すぎるクエリ（INV-GQL-06）
        let deep = format!("{{ postList(locale: \"ja\") {{ {}id{} }} }}", "related { ".repeat(24), " }".repeat(24));
        let wide = format!("{{ {} }}", (0..1000).map(|i| format!("a{i}: postList(locale: \"ja\") {{ id }}")).collect::<Vec<_>>().join(" "));
        for bad in [
            "{ nope }".to_owned(),
            "query A { postList(locale: \"ja\") { id } } query B { postList(locale: \"ja\") { id } }".to_owned(),
            "mutation { x }".to_owned(),
            "{ postList(locale: \"ja\") { id ".to_owned(),
            deep,
            wide,
        ] {
            let res = register(&server, ws, &reg, &bad).await;
            assert_eq!(error(&res), (422, Some("invalid_query".into())), "{}", res.body);
        }
        let shallow = format!("{{ postList(locale: \"ja\") {{ {}id{} }} }}", "related { ".repeat(2), " }".repeat(2));
        assert_eq!(register(&server, ws, &reg, &shallow).await.status, 201);

        // 実行: 公開版だけを返す（INV-PUB-01）
        let res = run(&server, ws, &read, &by_id_ref, json!({"id": ja, "locale": "ja"})).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert_eq!(res.header("cache-control"), Some("private, no-cache"), "INV-GQL-08");
        let post = &parse(&res)["data"]["post"];
        assert_eq!(post["title"], "こんにちは");
        assert_eq!(post["url"], "https://site.example.test/ja/blog/hello");
        assert_eq!(post["alternates"], json!([{"locale": "en", "url": "https://site.example.test/en/blog/hello"}]));
        assert_eq!(post["related"], json!([{"id": dep_id, "title": "依存先"}]));
        assert_eq!(post["body"]["content"][0]["content"][0]["text"], "PUBLISHED-BODY");
        let image = &post["body"]["content"][1]["attrs"];
        assert_eq!((image["width"].as_i64(), image["height"].as_i64()), (Some(3), Some(2)));
        assert!(image["src"].as_str().unwrap().starts_with("https://yomi.example.test/media/"));
        assert!(!res.body.contains("DRAFT-ONLY"));
        // 違うロケールを指定したら返らない
        assert_eq!(parse(&run(&server, ws, &read, &by_id_ref, json!({"id": ja, "locale": "en"})).await)["data"]["post"], Value::Null);
        // 一覧とスラッグ
        let list = "query($locale: String!, $limit: Int) { postList(locale: $locale, limit: $limit) { id title } }";
        let list_ref = register(&server, ws, &reg, list).await.json_str("id").unwrap();
        let res = run(&server, ws, &read, &list_ref, json!({"locale": "ja", "limit": 100})).await;
        let titles: Vec<String> = parse(&res)["data"]["postList"].as_array().unwrap().iter().map(|p| p["title"].as_str().unwrap().to_owned()).collect();
        assert_eq!(titles, ["こんにちは", "依存先"], "newest first; drafts are not listed");
        let res = run(&server, ws, &read, &list_ref, json!({"locale": "ja", "limit": 2_147_483_647})).await;
        assert!(parse(&res)["data"]["postList"].as_array().unwrap().len() <= 5, "listLimit caps the count (INV-GQL-05)");
        let slug = "query { postBySlug(slug: \"hello\", locale: \"en\") { title } }";
        let slug_ref = register(&server, ws, &reg, slug).await.json_str("id").unwrap();
        assert_eq!(parse(&run(&server, ws, &read, &slug_ref, json!({})).await)["data"]["postBySlug"]["title"], "Hello");

        // 非公開にすると、その直後から返らない（INV-PUB-02）。参照と他のロケールへのリンクからも消える（INV-LOC-03, 04）
        assert_eq!(send(&server, "POST", &format!("{entries}/{dep_id}/unpublish"), &editor, None).await.status, 204);
        assert_eq!(send(&server, "POST", &format!("{entries}/{en}/unpublish"), &editor, None).await.status, 204);
        let post = parse(&run(&server, ws, &read, &by_id_ref, json!({"id": ja})).await)["data"]["post"].clone();
        assert_eq!((post["related"].clone(), post["alternates"].clone()), (json!([]), json!([])));
        assert_eq!(parse(&run(&server, ws, &read, &slug_ref, json!({})).await)["data"]["postBySlug"], Value::Null);

        // 登録していないクエリ・存在しないID・一括送信は拒否（INV-GQL-01, 04）
        let auth = [("Authorization", format!("Bearer {read}"))];
        let auth: Vec<(&str, &str)> = auth.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let res = request(&server, "POST", &format!("/ws/{ws}/graphql"), &auth, Some(&json!({"query": list, "variables": {"locale": "ja"}}).to_string())).await;
        assert_eq!(error(&res), (400, Some("persisted_query_required".into())));
        assert!(!res.body.contains("こんにちは"));
        let res = run(&server, ws, &read, &"0".repeat(64), json!({})).await;
        assert_eq!(error(&res), (400, Some("persisted_query_not_found".into())));
        let res = request(&server, "POST", &format!("/ws/{ws}/graphql"), &auth, Some(&json!([{"id": list_ref}, {"id": by_id_ref}]).to_string())).await;
        assert_eq!(error(&res), (400, Some("batch_not_supported".into())));
        // POSTでも実行できる
        let res = request(&server, "POST", &format!("/ws/{ws}/graphql"), &auth, Some(&json!({"id": list_ref, "variables": {"locale": "ja"}}).to_string())).await;
        assert!(res.body.contains("こんにちは"), "{}", res.body);

        // キー: なし・登録用だけ・他のワークスペースのキー・URLのキーは拒否（INV-API-01, 02、INV-GQL-07）
        assert_eq!(request(&server, "GET", &format!("/ws/{ws}/graphql?id={list_ref}"), &[], None).await.status, 401);
        assert_eq!(run(&server, ws, &reg, &list_ref, json!({"locale": "ja"})).await.status, 403);
        let res = run(&server, ws, &other_read, &list_ref, json!({"locale": "ja"})).await;
        assert_eq!(res.status, 404);
        assert!(!res.body.contains("こんにちは"));
        let res = run(&server, other_ws, &read, &list_ref, json!({"locale": "ja"})).await;
        assert_eq!(res.status, 404);
        let res = request(&server, "GET", &format!("/ws/{ws}/graphql?id={list_ref}&api_key={read}"), &[], None).await;
        assert_eq!(res.status, 401);
        // 他のワークスペースでは、同じIDのクエリも登録されていない
        let other_list = register(&server, other_ws, &other_read, list).await.json_str("id").unwrap();
        assert_eq!(other_list, list_ref);
        let res = run(&server, other_ws, &other_read, &list_ref, json!({"locale": "ja"})).await;
        assert_eq!(parse(&res)["data"]["postList"], json!([]));

        // イントロスペクションは結果を返さない（INV-GQL-04）
        let intro = "{ __schema { types { name } } }";
        let res = register(&server, ws, &reg, intro).await;
        if res.status == 201 {
            let res = run(&server, ws, &read, &res.json_str("id").unwrap(), json!({})).await;
            assert!(!res.body.contains("\"types\""), "{}", res.body);
        }
        // 変数の既定値（ロケールを省略した一覧）
        let defaulted = "query($locale: String! = \"ja\", $limit: Int) { postList(locale: $locale, limit: $limit) { id title } }";
        let defaulted_ref = register(&server, ws, &reg, defaulted).await.json_str("id").unwrap();
        assert!(run(&server, ws, &read, &defaulted_ref, json!({})).await.body.contains("こんにちは"));
        // 削除した記事は返らない
        assert_eq!(send(&server, "DELETE", &format!("{entries}/{ja}"), &editor, None).await.status, 204);
        assert_eq!(parse(&run(&server, ws, &read, &by_id_ref, json!({"id": ja})).await)["data"]["post"], Value::Null);
    })
    .await;
}
