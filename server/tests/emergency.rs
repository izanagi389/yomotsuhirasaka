//! 緊急停止のCLIの結合テスト（本物のPostgreSQLとHTTP、ビルドしたサーバーのバイナリを使う）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::server::*;
use common::with_scratch;
use uuid::Uuid;
use yomi_server::api_keys::{self, Denied};
use yomi_server::auth::personal_tokens;

async fn user_id(server: &Server, email: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM yomi.users WHERE email = $1")
        .bind(email)
        .fetch_one(&server.pool)
        .await
        .unwrap()
}

async fn workspace(server: &Server, root: &Login, name: &str) -> Uuid {
    let res = send(
        server,
        "POST",
        "/api/admin/workspaces",
        root,
        Some(&format!("{{\"name\":\"{name}\"}}")),
    )
    .await;
    assert_eq!(res.status, 201, "{}", res.body);
    res.json_str("id").unwrap().parse().unwrap()
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

#[tokio::test]
async fn emergency_cli() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        create_user(&server.pool, "root@example.test", true).await;
        create_user(&server.pool, "user@example.test", false).await;
        let root = login(&server, "root@example.test", PASSWORD).await;
        enroll(&server, &root).await;
        let user = login(&server, "user@example.test", PASSWORD).await;
        let user_id = user_id(&server, "user@example.test").await;
        let a = workspace(&server, &root, "A").await;
        let b = workspace(&server, &root, "B").await;
        let token = personal_tokens::issue(
            &server.pool,
            user_id,
            "cli",
            &["read".to_owned()],
            &[a],
            "2999-01-01T00:00:00Z",
        )
        .await
        .unwrap()
        .secret;
        let (_, pending) = {
            // 多要素認証の途中のセッションも失効させる対象
            let s = login(&server, "user@example.test", PASSWORD).await;
            enroll(&server, &s).await;
            password_step(&server, "user@example.test", PASSWORD).await
        };
        let pending = pending.unwrap();
        let key_a = api_keys::issue(
            &server.pool,
            a,
            "site",
            &["read".to_owned()],
            "build",
            user_id,
            None,
        )
        .await
        .unwrap();
        let key_b = api_keys::issue(
            &server.pool,
            b,
            "site\tb",
            &["read".to_owned()],
            "build",
            user_id,
            None,
        )
        .await
        .unwrap();
        let me = |auth: String| {
            let server = &server;
            async move {
                let header = if auth.starts_with("Bearer ") {
                    "Authorization"
                } else {
                    "Cookie"
                };
                request(server, "GET", "/api/admin/me", &[(header, &auth)], None)
                    .await
                    .status
            }
        };
        assert_eq!(me(root.cookie_header()).await, 200);
        assert_eq!(me(bearer(&token)).await, 200);

        // CLIの設定
        let dir =
            std::env::temp_dir().join(format!("yomi-cli-emergency-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = dir.join("encryption.key");
        std::fs::write(&key, yomi_server::keys::generate_key_file()).unwrap();
        let mut cfg = config(&app, &migration);
        cfg.trusted_proxies = Vec::new();
        cfg.encryption_key_file = Some(key);
        let path = dir.join("config.toml");
        std::fs::write(&path, toml::to_string(&cfg).unwrap()).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new(env!("CARGO_BIN_EXE_yomi-server"))
                .args(args)
                .arg("--config")
                .arg(&path)
                .output()
                .unwrap()
        };

        // 確認（--yes）がなければ何もしない
        let out = run(&["revoke-all-sessions"]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("--yes"));
        assert_eq!(me(root.cookie_header()).await, 200);

        // 一斉失効（INV-AUTH-23）
        let out = run(&["revoke-all-sessions", "--yes"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        for s in [&root, &user] {
            assert_eq!(me(s.cookie_header()).await, 401);
        }
        assert_eq!(me(bearer(&token)).await, 401);
        let res = mfa_step(&server, &pending, "{\"totpCode\":\"000000\"}").await;
        assert_eq!(res.status, 401);
        let audit: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM yomi.audit_log WHERE action = 'emergency.sessions_revoked'",
        )
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert_eq!(audit, 1);
        // 新しくログインすれば使える
        create_user(&server.pool, "fresh@example.test", false).await;
        let again = login(&server, "fresh@example.test", PASSWORD).await;
        assert_eq!(me(again.cookie_header()).await, 200);
        // セッションとトークンに関係のない、配信APIのキーはそのまま
        assert!(
            api_keys::authenticate(&server.pool, Some(&key_a.secret), a, "read")
                .await
                .unwrap()
                .is_ok()
        );

        // 配信APIのキーの入れ替え: ワークスペースを絞る
        let out = run(&["rotate-api-keys", "--yes", "--workspace", &a.to_string()]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8(out.stdout).unwrap();
        let lines: Vec<Vec<&str>> = stdout.lines().map(|l| l.split('\t').collect()).collect();
        assert_eq!(lines.len(), 1, "{stdout}");
        assert_eq!(
            lines[0][..3],
            [
                a.to_string().as_str(),
                key_a.id.to_string().as_str(),
                "site"
            ]
        );
        let new_a = lines[0][3].to_owned();
        assert!(new_a.starts_with("yomi_key_") && new_a != key_a.secret);
        // 古い値は使えず、新しい値は同じ範囲で使える
        assert_eq!(
            api_keys::authenticate(&server.pool, Some(&key_a.secret), a, "read")
                .await
                .unwrap()
                .unwrap_err(),
            Denied::Invalid
        );
        let grant = api_keys::authenticate(&server.pool, Some(&new_a), a, "read")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(grant.key_id, key_a.id);
        assert!(
            api_keys::authenticate(&server.pool, Some(&key_b.secret), b, "read")
                .await
                .unwrap()
                .is_ok()
        );

        // すべてのワークスペース。名前の中の区切り文字で列がずれない
        let out = run(&["rotate-api-keys", "--yes"]);
        assert!(out.status.success());
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert_eq!(stdout.lines().count(), 2, "{stdout}");
        assert!(
            stdout.lines().all(|l| l.split('\t').count() == 4),
            "{stdout}"
        );
        assert!(stdout.contains("site b"));
        assert_eq!(
            api_keys::authenticate(&server.pool, Some(&key_b.secret), b, "read")
                .await
                .unwrap()
                .unwrap_err(),
            Denied::Invalid
        );
        assert_eq!(
            api_keys::authenticate(&server.pool, Some(&new_a), a, "read")
                .await
                .unwrap()
                .unwrap_err(),
            Denied::Invalid
        );
        let rotated: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM yomi.audit_log WHERE action = 'api_key.rotated'",
        )
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert_eq!(rotated, 3);

        let _ = std::fs::remove_dir_all(&dir);
    })
    .await;
}
