//! テンプレートのデプロイでの、登録済みのクエリの検証の結合テスト（本物のPostgreSQLとHTTPを使う。Q54、INV-SCH-03）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::{Value, json};
use uuid::Uuid;

/// 投稿の型からフィールドを除いたマニフェスト。
fn without(fields: &[&str]) -> String {
    let mut m: Value = serde_json::from_str(&blog_manifest()).unwrap();
    m["contentTypes"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .retain(|f| !fields.contains(&f["name"].as_str().unwrap()));
    m.to_string()
}

async fn api_key(server: &Server, root: &Login, ws: Uuid) -> String {
    let res = send(
        server,
        "POST",
        &format!("/api/admin/workspaces/{ws}/api-keys"),
        root,
        Some("{\"scopes\":[\"read\",\"register\"]}"),
    )
    .await;
    assert_eq!(res.status, 201, "{}", res.body);
    res.json_str("key").unwrap()
}

async fn register(server: &Server, ws: Uuid, key: &str, query: &str) -> String {
    let res = request(
        server,
        "POST",
        &format!("/ws/{ws}/graphql/queries"),
        &[("Authorization", &format!("Bearer {key}"))],
        Some(&json!({"query": query}).to_string()),
    )
    .await;
    assert_eq!(res.status, 201, "{}", res.body);
    res.json_str("id").unwrap()
}

async fn run(server: &Server, ws: Uuid, key: &str, id: &str) -> Value {
    let res = request(
        server,
        "GET",
        &format!("/ws/{ws}/graphql?id={id}"),
        &[("Authorization", &format!("Bearer {key}"))],
        None,
    )
    .await;
    assert_eq!(res.status, 200, "{}", res.body);
    serde_json::from_str(&res.body).unwrap()
}

#[tokio::test]
async fn deployments_that_break_registered_queries_are_refused() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(
            deploy(&server, &token, "blog", &blog_manifest(), "")
                .await
                .status,
            200
        );
        let a = workspace_with_template(&server, &root, "A", "blog").await;
        let b = workspace_with_template(&server, &root, "B", "blog").await;
        let key_a = api_key(&server, &root, a).await;
        let key_b = api_key(&server, &root, b).await;
        // A は subtitle を使うクエリ、B は related を使うクエリを登録する
        let uses_subtitle = register(
            &server,
            a,
            &key_a,
            "{ postList(locale: \"ja\") { id subtitle } }",
        )
        .await;
        register(
            &server,
            b,
            &key_b,
            "{ postList(locale: \"ja\") { id related { id } } }",
        )
        .await;

        // 破壊的な変更を許しても、登録済みのクエリを壊すなら拒否する
        let destructive = ",\"allowDestructive\":true";
        let res = deploy(
            &server,
            &token,
            "blog",
            &without(&["subtitle"]),
            destructive,
        )
        .await;
        assert_eq!(
            (res.status, res.json_str("error").as_deref()),
            (409, Some("breaks_registered_queries")),
            "{}",
            res.body
        );
        assert!(
            res.body.contains(&a.to_string()) && res.body.contains(&uses_subtitle),
            "{}",
            res.body
        );
        let res = deploy(&server, &token, "blog", &without(&["related"]), destructive).await;
        assert_eq!(
            res.status, 409,
            "a query of another workspace on the same template"
        );
        // 拒否の後も、スキーマは変わらず、クエリは動く
        assert!(run(&server, a, &key_a, &uses_subtitle).await["errors"].is_null());

        // どのクエリも使っていないフィールドの削除は通る
        let res = deploy(&server, &token, "blog", &without(&["body"]), destructive).await;
        assert_eq!(res.status, 200, "{}", res.body);

        // 長く使われていないクエリは検証しない（Q151）
        let schema = format!("ws_{}", a.simple());
        sqlx::query(cms_server::db::trusted(format!(
            "UPDATE {schema}.queries SET last_used_at = now() - interval '91 days'"
        )))
        .execute(&server.state.migration)
        .await
        .unwrap();
        let res = deploy(
            &server,
            &token,
            "blog",
            &without(&["body", "subtitle"]),
            destructive,
        )
        .await;
        assert_eq!(res.status, 200, "{}", res.body);
        // 明示的に許せば、壊れるクエリがあってもデプロイできる
        let res = deploy(
            &server,
            &token,
            "blog",
            &without(&["body", "subtitle", "related"]),
            &format!("{destructive},\"allowBrokenQueries\":true"),
        )
        .await;
        assert_eq!(res.status, 200, "{}", res.body);
        let out = run(&server, a, &key_a, &uses_subtitle).await;
        assert!(
            !out["errors"].is_null(),
            "the broken query now fails: {out}"
        );
    })
    .await;
}
