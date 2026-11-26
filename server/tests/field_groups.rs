//! グループのフィールドの検索・AI向けのMarkdown版・訂正と削除済みアンカーの結合テスト（2026-11-25、Q11・Q12）。
//!
//! 環境変数 `YOMI_TEST_PG_BIGM_URL` に、`pg_bigm` と pgvector の入った PostgreSQL のスーパーユーザーの接続先があるときだけ
//! 実行する（キーワード検索で、グループの件のチャンクを確かめるため）。LLMの接続先は設定しない。

mod common;

use common::content::*;
use common::server::*;
use common::with_scratch_at;
use serde_json::{Value, json};
use uuid::Uuid;
use yomi_server::worker;

fn parse(res: &Resp) -> Value {
    serde_json::from_str(&res.body).unwrap_or_else(|_| panic!("{}", res.body))
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

/// FAQ のグループを持ち、その中の質問と答えを検索の対象にし、Markdown版に載せるテンプレート。
fn manifest() -> String {
    let m = blog_manifest().replace(
        r#"{"name": "related", "type": "reference", "referenceTo": "post", "multiple": true}"#,
        r#"{"name": "related", "type": "reference", "referenceTo": "post", "multiple": true},
           {"name": "faq", "type": "group", "fields": [
             {"name": "question", "type": "text", "searchable": true},
             {"name": "answer", "type": "text", "searchable": true},
             {"name": "note", "type": "text"},
             {"name": "level", "type": "select", "options": [{"value": "basic"}, {"value": "advanced"}]}
           ]}"#,
    );
    let m = m.replace(
        r#""listLimit": 5,"#,
        r#""listLimit": 5, "aiMarkdown": {"title": "title", "fields": ["body", "faq"]},"#,
    );
    assert_ne!(m, blog_manifest());
    m
}

const SEARCH: &str = "query($text: String!) { search(text: $text, locale: \"ja\", first: 50, mode: KEYWORD) { keywordSearch hits { entryId anchor } } }";
const MARKDOWN: &str = "query($id: ID!) { post(id: $id, locale: \"ja\") { aiMarkdown } }";

#[tokio::test]
async fn group_items_are_searched_rendered_and_tracked_as_sections() {
    with_scratch_at("YOMI_TEST_PG_BIGM_URL", &["pg_bigm"], |app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        let res = deploy(&server, &token, "blog", &manifest(), "").await;
        assert_eq!(res.status, 200, "{}", res.body);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        create_user(&server.pool, "editor@example.test", false).await;
        add_member(&server, ws, "editor@example.test", "editor").await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let entries = format!("/api/admin/workspaces/{ws}/entries");
        let read = api_key(&server, &root, ws, &["read"]).await;
        let reg = api_key(&server, &root, ws, &["register"]).await;

        // 2件の FAQ を持つ記事を公開する。件には、サーバーがIDを振る
        let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "本文です。"}]}]});
        let faq = json!([
            {"question": "全文検索の質問", "answer": "索引を使います。", "note": "覚え書きは検索しない", "level": "basic"},
            {"question": "消す質問", "answer": "後で消します。"}
        ]);
        let res = send(&server, "POST", &entries, &editor, Some(&json!({"contentType": "post", "fields": {"title": "FAQ", "slug": "faq", "body": body, "faq": faq}}).to_string())).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let created = parse(&res);
        let entry = format!("{entries}/{}", created["id"].as_str().unwrap());
        let item_id = |i: usize| created["fields"]["faq"][i]["id"].as_str().unwrap().to_owned();
        let (kept, removed) = (item_id(0), item_id(1));
        assert!(kept.starts_with("i-") && removed.starts_with("i-"));
        assert_eq!(send(&server, "POST", &format!("{entry}/publish"), &editor, Some("{\"revision\":1}")).await.status, 200);
        while worker::run_once(&server.state).await.unwrap() {}

        let register = |query: &'static str| {
            let (server, reg) = (&server, &reg);
            async move {
                let res = request(server, "POST", &format!("/ws/{ws}/graphql/queries"), &[("Authorization", &format!("Bearer {reg}"))], Some(&json!({"query": query}).to_string())).await;
                assert_eq!(res.status, 201, "{}", res.body);
                res.json_str("id").unwrap()
            }
        };
        let (search_query, markdown_query) = (register(SEARCH).await, register(MARKDOWN).await);
        let run = |query: String, vars: Value| {
            let (server, read) = (&server, &read);
            async move {
                let body = json!({"id": query, "variables": vars}).to_string();
                let res = request(server, "POST", &format!("/ws/{ws}/graphql"), &[("Authorization", &format!("Bearer {read}"))], Some(&body)).await;
                assert_eq!(res.status, 200, "{}", res.body);
                let body = parse(&res);
                assert!(body.get("errors").is_none(), "{}", res.body);
                body["data"].clone()
            }
        };

        // 検索: 件が1つのチャンクになり、アンカーは件のID。検索の対象でない中のフィールドは当たらない
        let r = run(search_query.clone(), json!({"text": "全文検索"})).await;
        assert_eq!(r["search"]["keywordSearch"], "AVAILABLE");
        assert_eq!(r["search"]["hits"][0]["anchor"], json!(kept), "{r}");
        let r = run(search_query.clone(), json!({"text": "覚え書き"})).await;
        assert_eq!(r["search"]["hits"], json!([]), "検索の対象でないフィールドが当たった");

        // Markdown版: グループの節と、件ごとの見出し（アンカーは件のID）と「名前: 値」の行
        let id = created["id"].clone();
        let md = run(markdown_query.clone(), json!({"id": id})).await["post"]["aiMarkdown"].as_str().unwrap().to_owned();
        assert!(md.contains("\n## faq\n"), "{md}");
        assert!(md.contains(&format!("### 全文検索の質問 {{#{kept}}}")), "{md}");
        assert!(md.contains("- answer: 索引を使います。"), "{md}");
        assert!(md.contains("- level: basic"), "{md}");
        assert!(md.contains(&format!("### 消す質問 {{#{removed}}}")), "{md}");

        // 1件の答えを直し、もう1件を消して、訂正として公開する。どちらも影響したセクションに数え、消した件は削除済みアンカーになる
        let draft = parse(&get(&server, &entry, &editor).await);
        let mut first = created["fields"]["faq"][0].clone();
        first["answer"] = json!("索引を2つ使います。");
        let save = json!({"revision": draft["revision"], "fields": {"faq": [first]}}).to_string();
        let res = send(&server, "PUT", &format!("{entry}/draft"), &editor, Some(&save)).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let saved = parse(&res);
        assert_eq!(saved["fields"]["faq"][0]["id"], json!(kept), "送ったIDは変わらない");
        let publish = json!({"revision": saved["revision"], "correction": true}).to_string();
        let res = send(&server, "POST", &format!("{entry}/publish"), &editor, Some(&publish)).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let versions = parse(&get(&server, &format!("{entry}/versions"), &editor).await);
        let mut affected: Vec<String> = versions[1]["affectedAnchors"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        affected.sort();
        let mut expected = vec![kept.clone(), removed.clone()];
        expected.sort();
        assert_eq!(affected, expected);

        let md = run(markdown_query, json!({"id": id})).await["post"]["aiMarkdown"].as_str().unwrap().to_owned();
        let front = md.split("\n---\n").next().unwrap();
        assert!(front.contains("deletedAnchors") && front.contains(&removed), "{md}");
        assert!(!md.contains("消す質問"), "{md}");
        assert!(md.contains("- answer: 索引を2つ使います。"), "{md}");
    })
    .await;
}
