//! ワークスペースの削除の結合テスト（11-permissions-and-audit.md「ワークスペースと利用者」、INV-TEN-07・08、INV-DB-05）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use std::io::Cursor;

use common::content::*;
use common::server::*;
use common::with_scratch;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use uuid::Uuid;
use yomi_server::workspace_db::WorkspaceDb;

fn png() -> Vec<u8> {
    let img = RgbImage::from_fn(8, 8, |x, y| Rgb([x as u8 * 30, y as u8 * 30, 7]));
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(img)
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

async fn exists(server: &Server, sql: &'static str, name: &str) -> bool {
    sqlx::query_scalar::<_, bool>(sql)
        .bind(name)
        .fetch_one(&server.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn server_admins_delete_workspaces_completely() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        create_user(&server.pool, "root@example.test", true).await;
        let root = login(&server, "root@example.test", PASSWORD).await;
        let (secret, _) = enroll(&server, &root).await;
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
        let ws = workspace_with_template(&server, &root, "Doomed", "blog").await;
        let other = workspace_with_template(&server, &root, "Other", "blog").await;
        create_user(&server.pool, "admin@example.test", false).await;
        add_member(&server, ws, "admin@example.test", "admin").await;
        add_member(&server, other, "admin@example.test", "admin").await;
        let admin = login(&server, "admin@example.test", PASSWORD).await;
        enroll(&server, &admin).await;
        let res = send(
            &server,
            "POST",
            "/api/admin/me/tokens",
            &admin,
            Some(&format!(r#"{{"name":"rw","scopes":["read"],"workspaceIds":["{ws}","{other}"]}}"#)),
        )
        .await;
        assert_eq!(res.status, 201, "{}", res.body);
        let entries = format!("/api/admin/workspaces/{ws}/entries");
        let res = send(&server, "POST", &entries, &root, Some(r#"{"contentType":"post","fields":{"title":"t","slug":"s"}}"#)).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let res = upload(&server, &format!("/api/admin/workspaces/{ws}/media"), &root, "image/png", &png()).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let media_dir = server.state.config.media.root.join(ws.simple().to_string());
        assert!(media_dir.exists());

        let path = format!("/api/admin/workspaces/{ws}");
        let confirm = r#"{"confirmName":"Doomed"}"#;
        // サーバー管理者だけ
        assert_eq!(send(&server, "DELETE", &path, &admin, Some(confirm)).await.status, 403);
        // 名前の入力が要る
        for bad in ["{}", r#"{"confirmName":"Other"}"#, r#"{"confirmName":"doomed"}"#] {
            let res = send(&server, "DELETE", &path, &root, Some(bad)).await;
            assert_eq!((res.status, res.json_str("error").as_deref()), (422, Some("confirm_name_mismatch")), "{bad}: {}", res.body);
        }
        // 再認証が要る
        expire_reauth(&server.pool, "root@example.test").await;
        let res = send(&server, "DELETE", &path, &root, Some(confirm)).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (403, Some("reauth_required")), "{}", res.body);
        forget_used_steps(&server.pool, "root@example.test").await;
        let res = send(&server, "POST", "/api/admin/session/reauth", &root, Some(&format!("{{\"totpCode\":\"{}\"}}", code(&secret, 0)))).await;
        assert!(res.status < 300, "{}", res.body);

        let res = send(&server, "DELETE", &path, &root, Some(confirm)).await;
        assert_eq!(res.status, 204, "{}", res.body);
        // もう一度は見つからない
        assert_eq!(send(&server, "DELETE", &path, &root, Some(confirm)).await.status, 404);
        assert_eq!(get(&server, &path, &root).await.status, 404);

        // スキーマ・ロール・共通のテーブルの行・メディアのファイルが消える
        let db = WorkspaceDb::names_for(ws);
        assert!(!exists(&server, "SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = $1)", db.schema.as_str()).await);
        assert!(!exists(&server, "SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = $1)", db.role.as_str()).await);
        let memberships: i64 = sqlx::query_scalar("SELECT count(*) FROM yomi.memberships WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(memberships, 0);
        assert!(!media_dir.exists());
        // 利用者は残り、個人トークンからはそのワークスペースだけが外れる
        let tokens: Vec<Vec<Uuid>> = sqlx::query_scalar("SELECT workspace_ids FROM yomi.personal_tokens WHERE name = 'rw'")
            .fetch_all(&server.pool)
            .await
            .unwrap();
        assert_eq!(tokens, vec![vec![other]]);
        assert_eq!(get(&server, &format!("/api/admin/workspaces/{other}"), &admin).await.status, 200);

        // 監査ログ: ワークスペースの行は残り、削除はインスタンス全体の記録として管理者に知らせる
        let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM yomi.audit_log WHERE workspace_id = $1 AND action = 'workspace.created'")
            .bind(ws)
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(kept, 1);
        let deleted: Vec<(Option<Uuid>, Vec<String>, bool)> = sqlx::query_as(
            "SELECT workspace_id, target_ids, notify FROM yomi.audit_log WHERE action = 'workspace.deleted'",
        )
        .fetch_all(&server.pool)
        .await
        .unwrap();
        assert_eq!(deleted, vec![(None, vec![ws.to_string()], true)]);

        // 同じ名前で作り直せる
        let again = workspace_with_template(&server, &root, "Doomed", "blog").await;
        assert_ne!(again, ws);
    })
    .await;
}
