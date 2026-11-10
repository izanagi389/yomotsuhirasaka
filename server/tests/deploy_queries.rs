//! テンプレートのデプロイでの、登録済みのクエリの検証の結合テスト（本物のPostgreSQLとHTTPを使う。Q54、INV-SCH-03）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

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

        // 破壊的な変更を許しても、登録済みのクエリを壊すワークスペースには適用しない。
        // ほかのワークスペースには適用する（1つのワークスペースのクエリで、全体のデプロイを止めさせない。R42）
        let destructive = ",\"allowDestructive\":true";
        let res = deploy(
            &server,
            &token,
            "blog",
            &without(&["subtitle"]),
            destructive,
        )
        .await;
        assert_eq!(res.status, 200, "{}", res.body);
        let out: Value = serde_json::from_str(&res.body).unwrap();
        let result_of = |ws: Uuid| {
            out["workspaces"]
                .as_array()
                .unwrap()
                .iter()
                .find(|w| w["workspaceId"] == ws.to_string())
                .cloned()
                .unwrap_or_else(|| panic!("{out}"))
        };
        let held = result_of(a);
        assert_eq!(held["state"], "pending", "{out}");
        assert_eq!(held["error"], "breaks_registered_queries", "{out}");
        assert!(
            held["brokenQueries"].to_string().contains(&uses_subtitle),
            "{out}"
        );
        assert_eq!(result_of(b)["state"], "ok", "{out}");
        assert!(result_of(b).get("brokenQueries").is_none(), "{out}");
        // 適用しなかった側では、クエリが動く
        assert!(run(&server, a, &key_a, &uses_subtitle).await["errors"].is_null());
        let held_state: String =
            sqlx::query_scalar("SELECT schema_state FROM yomi.workspaces WHERE id = $1")
                .bind(a)
                .fetch_one(&server.pool)
                .await
                .unwrap();
        assert_eq!(held_state, "pending");

        // どのクエリも使っていないフィールドの削除は通り、移行待ちだった側も、クエリが壊れなければ移る
        let res = deploy(&server, &token, "blog", &without(&["body"]), destructive).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let state: String =
            sqlx::query_scalar("SELECT schema_state FROM yomi.workspaces WHERE id = $1")
                .bind(a)
                .fetch_one(&server.pool)
                .await
                .unwrap();
        assert_eq!(state, "ok", "{}", res.body);

        // 長く使われていないクエリは検証しない（Q151）
        let schema = format!("ws_{}", a.simple());
        sqlx::query(yomi_server::db::trusted(format!(
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

#[tokio::test]
async fn dry_run_reports_without_changing_anything() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        let dry = ",\"dryRun\":true";

        // まだ版がないテンプレート: 何も書かない
        let res = deploy(&server, &token, "blog", &blog_manifest(), dry).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let c: Value = serde_json::from_str(&res.body).unwrap();
        assert_eq!(c["dryRun"], true);
        assert!(c["currentVersion"].is_null());
        assert_eq!(c["problems"], json!([]));
        let versions = || async {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM yomi.template_versions")
                .fetch_one(&server.pool)
                .await
                .unwrap()
        };
        assert_eq!(versions().await, 0);

        assert_eq!(
            deploy(&server, &token, "blog", &blog_manifest(), "")
                .await
                .status,
            200
        );
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let key = api_key(&server, &root, ws).await;
        let uses_subtitle = register(
            &server,
            ws,
            &key,
            "{ postList(locale: \"ja\") { id subtitle } }",
        )
        .await;

        // 同じ内容
        let c: Value = serde_json::from_str(
            &deploy(&server, &token, "blog", &blog_manifest(), dry)
                .await
                .body,
        )
        .unwrap();
        assert_eq!(
            (
                c["unchanged"].as_bool(),
                c["currentVersion"].as_i64(),
                c["workspaces"].as_i64()
            ),
            (Some(true), Some(1), Some(1))
        );

        // 破壊的な変更と、壊れるクエリ: 指定がなくても、すべて報告する。何も書かない
        let res = deploy(&server, &token, "blog", &without(&["subtitle"]), dry).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let c: Value = serde_json::from_str(&res.body).unwrap();
        assert_eq!(
            c["destructive"],
            json!(["field `post.subtitle` is removed"])
        );
        assert_eq!(c["brokenQueries"][0]["queryId"], uses_subtitle.as_str());
        assert_eq!(c["brokenQueries"][0]["workspaceId"], ws.to_string());
        assert_eq!(versions().await, 1);
        assert!(run(&server, ws, &key, &uses_subtitle).await["errors"].is_null());

        // マニフェストの問題は 422 ではなく、結果の中に返す
        let mut bad: Value = serde_json::from_str(&blog_manifest()).unwrap();
        bad["defaultLocale"] = json!("fr");
        let c: Value = serde_json::from_str(
            &deploy(&server, &token, "blog", &bad.to_string(), dry)
                .await
                .body,
        )
        .unwrap();
        assert!(
            c["problems"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p.as_str().unwrap().starts_with("defaultLocale")),
            "{c}"
        );

        // 試行も、デプロイの範囲のトークンが要る
        let res = request(
            &server,
            "POST",
            "/api/admin/templates/blog/deploy",
            &[
                ("Cookie", &root.cookie_header()),
                ("X-CSRF-Token", &root.csrf),
            ],
            Some(&format!("{{\"manifest\":{}{dry}}}", blog_manifest())),
        )
        .await;
        assert_eq!(res.status, 401, "{}", res.body);
        // 監査ログには、試行を記録しない（デプロイの1回だけ）
        let deployed: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM yomi.audit_log WHERE action = 'template.deployed'",
        )
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert_eq!(deployed, 1);
    })
    .await;
}

/// デプロイの途中で相手が接続を切っても、デプロイのロック（アドバイザリーロック）は残らない。
/// 残ると、移行用の接続が入れ替わった後のデプロイとワークスペースの作成が、すべて待ち続ける。
#[tokio::test]
async fn a_dropped_deploy_request_releases_the_lock() {
    use tokio::io::AsyncWriteExt as _;
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        // 移行するワークスペースを増やして、ロックを持つ時間を延ばす
        for name in ["A", "B", "C", "D", "E", "F", "G", "H"] {
            workspace_with_template(&server, &root, name, "blog").await;
        }
        let mut manifest: serde_json::Value = serde_json::from_str(&blog_manifest()).unwrap();
        for i in 0..30u64 {
            manifest["contentTypes"][0]["fields"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({"name": format!("extra{i}"), "type": "text"}));
            let body = serde_json::json!({"manifest": manifest}).to_string();
            let mut s = tokio::net::TcpStream::connect(server.addr).await.unwrap();
            let head = format!(
                "POST /api/admin/templates/blog/deploy HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            s.write_all(head.as_bytes()).await.unwrap();
            s.write_all(body.as_bytes()).await.unwrap();
            // 処理の途中で切る（時間をずらして、ロックを持っている間に当たるようにする）
            tokio::time::sleep(std::time::Duration::from_millis(i * 2)).await;
            drop(s);
        }
        // 走っているデプロイが終わるのを待つ
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let held: i64 =
            sqlx::query_scalar("SELECT count(*) FROM pg_locks WHERE locktype = 'advisory'")
                .fetch_one(&server.pool)
                .await
                .unwrap();
        assert_eq!(held, 0, "a deploy lock was left behind");
        // その後のデプロイも通る
        manifest["contentTypes"][0]["fields"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"name": "after", "type": "text"}));
        let res = deploy(&server, &token, "blog", &manifest.to_string(), "").await;
        assert_eq!(res.status, 200, "{}", res.body);
    })
    .await;
}
