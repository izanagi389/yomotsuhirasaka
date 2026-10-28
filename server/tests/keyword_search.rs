//! キーワード検索の結合テスト（08-search-and-related.md、M3）。pg_bigm の入った PostgreSQL を使う。
//!
//! 環境変数 `CMS_TEST_PG_BIGM_URL` に、pg_bigm と pgvector の入った PostgreSQL（`deploy/postgres.Dockerfile`）の
//! スーパーユーザーの接続先があるときだけ実行する。拡張がない環境の扱い（INV-SRC-05）は、約束のテストが確かめる。
//! LLMの接続先は設定しない（キーワード検索は、埋め込みがなくても動く）。

mod common;

use cms_server::worker;
use common::content::*;
use common::server::*;
use common::with_scratch_at;
use serde_json::{Value, json};
use uuid::Uuid;

fn parse(res: &Resp) -> Value {
    serde_json::from_str(&res.body).unwrap_or_else(|_| panic!("{}", res.body))
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

const SEARCH: &str = "query($text: String!, $mode: CmsSearchMode) { search(text: $text, locale: \"ja\", first: 50, mode: $mode) { vectorSearch keywordSearch hits { entryId excerpt matches { start length } score } } }";

/// 抜粋の中の、一致した箇所の文字列（位置は UTF-16 の単位）。
fn matched(hit: &Value) -> Vec<String> {
    let units: Vec<u16> = hit["excerpt"].as_str().unwrap().encode_utf16().collect();
    hit["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            let (s, l) = (
                m["start"].as_u64().unwrap() as usize,
                m["length"].as_u64().unwrap() as usize,
            );
            String::from_utf16(&units[s..s + l]).unwrap()
        })
        .collect()
}

fn ids(result: &Value) -> Vec<String> {
    result["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["entryId"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn keyword_search_with_pg_bigm() {
    with_scratch_at("CMS_TEST_PG_BIGM_URL", &["pg_bigm"], |app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        let manifest = blog_manifest().replace(
            r#"{"name": "body", "type": "richtext"}"#,
            r#"{"name": "body", "type": "richtext", "searchable": true}"#,
        );
        assert_ne!(manifest, blog_manifest());
        let res = deploy(&server, &token, "blog", &manifest, "").await;
        assert_eq!(res.status, 200, "{}", res.body);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        create_user(&server.pool, "editor@example.test", false).await;
        add_member(&server, ws, "editor@example.test", "editor").await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let entries = format!("/api/admin/workspaces/{ws}/entries");
        let read = api_key(&server, &root, ws, &["read"]).await;
        let reg = api_key(&server, &root, ws, &["register"]).await;

        // ワークスペースのチャンクの本文に、pg_bigm の索引ができている
        let indexes: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_indexes WHERE schemaname = $1 AND indexname = 'search_chunks_bigm'",
        )
        .bind(format!("ws_{}", ws.simple()))
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert_eq!(indexes, 1);

        let publish = |slug: &'static str, text: &'static str| {
            let (server, editor, entries) = (&server, &editor, &entries);
            async move {
                let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": text}]}]});
                let res = send(server, "POST", entries, editor, Some(&json!({"contentType": "post", "fields": {"title": slug, "slug": slug, "body": body}}).to_string())).await;
                let id = parse(&res)["id"].as_str().unwrap().to_owned();
                assert_eq!(send(server, "POST", &format!("{entries}/{id}/publish"), editor, Some("{\"revision\":1}")).await.status, 200);
                id
            }
        };
        let both = publish("both", "PostgreSQL の全文検索では、専用の索引を使います。").await;
        let one = publish("one", "全文検索の話だけをする記事です。").await;
        let percent = publish("percent", "成功の確率は 100% です。").await;
        let hidden = publish("hidden", "全文検索の記事ですが、非公開にします。").await;
        assert_eq!(send(&server, "POST", &format!("{entries}/{hidden}/unpublish"), &editor, None).await.status, 204);
        // チャンクを作る（LLMの接続先がなくても作る）
        while worker::run_once(&server.state).await.unwrap() {}

        let res = request(&server, "POST", &format!("/ws/{ws}/graphql/queries"), &[("Authorization", &format!("Bearer {reg}"))], Some(&json!({"query": SEARCH}).to_string())).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let query = res.json_str("id").unwrap();
        let search = |text: &'static str, mode: Option<&'static str>| {
            let (server, read, query) = (&server, &read, &query);
            async move {
                let vars = match mode {
                    Some(m) => json!({"text": text, "mode": m}),
                    None => json!({"text": text}),
                };
                let res = request(server, "GET", &format!("/ws/{ws}/graphql?id={query}&variables={}", enc(&vars.to_string())), &[("Authorization", &format!("Bearer {read}"))], None).await;
                assert_eq!(res.status, 200, "{}", res.body);
                let body = parse(&res);
                assert!(body.get("errors").is_none(), "{}", res.body);
                body["data"]["search"].clone()
            }
        };

        // すべての語を含む記事だけが当たる。キーワード検索は使え、ベクトル検索は（LLMの接続先がないので）無効
        let r = search("全文検索　索引", Some("KEYWORD")).await;
        assert_eq!(r["keywordSearch"], "AVAILABLE");
        assert_eq!(r["vectorSearch"], "DISABLED");
        assert_eq!(ids(&r), vec![both.clone()]);
        assert_eq!(matched(&r["hits"][0]), vec!["全文検索", "索引"], "一致した箇所の位置が抜粋とずれている");
        // 大文字・小文字は区別しない
        let r = search("postgresql", Some("KEYWORD")).await;
        assert_eq!(ids(&r), vec![both.clone()]);
        assert_eq!(matched(&r["hits"][0]), vec!["PostgreSQL"]);
        // 非公開にした記事は出ない（共通の層で照合する）
        let r = search("全文検索", Some("KEYWORD")).await;
        let mut found = ids(&r);
        found.sort();
        let mut expected = vec![both.clone(), one.clone()];
        expected.sort();
        assert_eq!(found, expected);
        // LIKE の記号は、そのままの文字として探す
        let r = search("%", Some("KEYWORD")).await;
        assert_eq!(ids(&r), vec![percent.clone()]);
        assert_eq!(search("_", Some("KEYWORD")).await["hits"], json!([]));
        // 既定（2つを混ぜる）でも、ベクトル検索が使えなければキーワード検索の結果を返す
        let r = search("全文検索 索引", None).await;
        assert_eq!(r["keywordSearch"], "AVAILABLE");
        assert_eq!(r["vectorSearch"], "DISABLED");
        assert_eq!(ids(&r), vec![both.clone()]);
        assert_eq!(matched(&r["hits"][0]), vec!["全文検索", "索引"]);
        // ベクトル検索だけを選ぶと、何も返らない（LLMの接続先がない）
        let r = search("全文検索", Some("VECTOR")).await;
        assert_eq!(r["vectorSearch"], "DISABLED");
        assert_eq!(r["hits"], json!([]));
    })
    .await;
}
