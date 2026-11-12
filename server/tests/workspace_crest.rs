//! ワークスペースの紋の結合テスト（ワークスペースの設定。持ち主の判断 2026-11-12）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::content::*;
use common::server::*;
use common::with_scratch;

fn crest_of(body: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(body).unwrap()["crest"].clone()
}

#[tokio::test]
async fn workspace_admins_choose_the_crest() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        create_user(&server.pool, "root@example.test", true).await;
        let root = login(&server, "root@example.test", PASSWORD).await;
        enroll(&server, &root).await;
        let res = send(
            &server,
            "POST",
            "/api/admin/me/tokens",
            &root,
            Some(r#"{"name":"deploy","scopes":["deploy"],"workspaceIds":[]}"#),
        )
        .await;
        let token = res.json_str("token").unwrap();
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "Crest", "blog").await;
        create_user(&server.pool, "admin@example.test", false).await;
        create_user(&server.pool, "editor@example.test", false).await;
        add_member(&server, ws, "admin@example.test", "admin").await;
        add_member(&server, ws, "editor@example.test", "editor").await;
        let admin = login(&server, "admin@example.test", PASSWORD).await;
        enroll(&server, &admin).await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        enroll(&server, &editor).await;

        let path = format!("/api/admin/workspaces/{ws}");
        let settings = format!("{path}/settings");
        // 選ぶ前は null（管理画面が ID から決める）
        assert_eq!(crest_of(&get(&server, &path, &admin).await.body), serde_json::Value::Null);

        // 編集者は変えられない
        let body = r#"{"crest":{"motif":"sakura","color":"shu"}}"#;
        assert_eq!(send(&server, "PATCH", &settings, &editor, Some(body)).await.status, 403);
        // 知らない形・色、片方だけは受け付けない
        for bad in [
            r#"{"crest":{"motif":"dragon","color":"shu"}}"#,
            r#"{"crest":{"motif":"sakura","color":"gold"}}"#,
            r#"{"crest":{"motif":"sakura"}}"#,
            r#"{"crest":{"motif":"sakura","color":"shu","extra":1}}"#,
        ] {
            let res = send(&server, "PATCH", &settings, &admin, Some(bad)).await;
            assert_eq!(res.status, 400, "{bad}: {}", res.body);
        }
        assert_eq!(crest_of(&get(&server, &path, &admin).await.body), serde_json::Value::Null);

        // 管理者が選ぶと、1件の取得と一覧の両方で返る
        let res = send(&server, "PATCH", &settings, &admin, Some(body)).await;
        assert!(res.status < 300, "{}", res.body);
        let chosen = serde_json::json!({"motif": "sakura", "color": "shu"});
        assert_eq!(crest_of(&get(&server, &path, &admin).await.body), chosen);
        let list: serde_json::Value =
            serde_json::from_str(&get(&server, "/api/admin/workspaces", &editor).await.body).unwrap();
        assert_eq!(list[0]["crest"], chosen);

        // ほかの設定だけを変えても、紋は変わらない
        let res = send(&server, "PATCH", &settings, &admin, Some(r#"{"takeOverPolicy":"permission"}"#)).await;
        assert!(res.status < 300, "{}", res.body);
        assert_eq!(crest_of(&get(&server, &path, &admin).await.body), chosen);

        // null で既定に戻す
        let res = send(&server, "PATCH", &settings, &admin, Some(r#"{"crest":null}"#)).await;
        assert!(res.status < 300, "{}", res.body);
        assert_eq!(crest_of(&get(&server, &path, &admin).await.body), serde_json::Value::Null);

        // 変更は監査ログに残る（設定の変更として）
        let changes: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM yomi.audit_log WHERE workspace_id = $1 AND action = 'workspace.settings_changed'",
        )
        .bind(ws)
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert_eq!(changes, 3);
    })
    .await;
}
