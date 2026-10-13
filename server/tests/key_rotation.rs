//! 暗号化の鍵の入れ替えのCLIの結合テスト（本物のPostgreSQLとHTTP、ビルドしたサーバーのバイナリを使う）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use cms_server::keys::KeyRing;
use cms_server::worker;
use common::server::*;
use common::smtp::*;
use common::with_scratch;
use uuid::Uuid;

#[tokio::test]
async fn rotating_the_encryption_key_keeps_stored_secrets_usable() {
    with_scratch(|app, migration| async move {
        let dir = std::env::temp_dir().join(format!("cms-cli-rekey-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = dir.join("encryption.key");
        std::fs::write(&key, cms_server::keys::generate_key_file()).unwrap();
        let (port, smtp) = fake_smtp().await;
        let mut cfg = config(&app, &migration);
        cfg.smtp = Some(smtp_config(port));
        cfg.encryption_key_file = Some(key.clone());

        // 鍵 v1 で、TOTPの秘密（設定済み・設定の途中）と、ジョブの中身を暗号化して保存する
        let before = start(cfg.clone()).await;
        create_user(&before.pool, "mfa@example.test", false).await;
        create_user(&before.pool, "pending@example.test", false).await;
        let s = login(&before, "mfa@example.test", PASSWORD).await;
        let (secret, _) = enroll(&before, &s).await;
        while worker::run_once(&before.state).await.unwrap() {}
        let p = login(&before, "pending@example.test", PASSWORD).await;
        assert_eq!(
            send(&before, "POST", "/api/admin/me/mfa/totp", &p, None)
                .await
                .status,
            200
        );
        // 送られていないメールのジョブ（パスワードのリセットの依頼）
        let res = request(
            &before,
            "POST",
            "/api/admin/password-resets",
            &[("X-Forwarded-For", "198.18.50.1")],
            Some("{\"email\":\"mfa@example.test\"}"),
        )
        .await;
        assert_eq!(res.status, 202);

        let mut cli_cfg = cfg.clone();
        cli_cfg.trusted_proxies = Vec::new();
        let path = dir.join("config.toml");
        std::fs::write(&path, toml::to_string(&cli_cfg).unwrap()).unwrap();
        let run = |args: &[&str]| {
            let out = std::process::Command::new(env!("CARGO_BIN_EXE_cms-server"))
                .args(args)
                .arg("--config")
                .arg(&path)
                .output()
                .unwrap();
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            (out.status.success(), text)
        };
        let versions = || KeyRing::load(&key).unwrap().versions();

        // 1. 新しい鍵を加える
        let (ok, out) = run(&["add-encryption-key"]);
        assert!(ok, "{out}");
        assert_eq!(versions(), vec![1, 2]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        // 古い鍵は、まだ使われているので外せない。現在の鍵も外せない。--yes がなければ何もしない
        let (ok, out) = run(&["remove-encryption-key", "--version", "1", "--yes"]);
        assert!(!ok && out.contains("still use key v1"), "{out}");
        let (ok, _) = run(&["remove-encryption-key", "--version", "2", "--yes"]);
        assert!(!ok);
        let (ok, _) = run(&["reencrypt-secrets"]);
        assert!(!ok);
        assert_eq!(versions(), vec![1, 2]);

        // 2. 暗号化し直す（2回目は何もしない）
        let (ok, out) = run(&["reencrypt-secrets", "--yes"]);
        assert!(ok, "{out}");
        assert!(
            out.contains("Re-encrypted 3 secrets") && out.contains("in use: v2: 3."),
            "{out}"
        );
        let (ok, out) = run(&["reencrypt-secrets", "--yes"]);
        assert!(ok && out.contains("Re-encrypted 0 secrets"), "{out}");

        // 3. 古い鍵を外す
        let (ok, out) = run(&["remove-encryption-key", "--version", "1", "--yes"]);
        assert!(ok, "{out}");
        assert_eq!(versions(), vec![2]);
        for action in [
            "encryption_key.added",
            "encryption_key.secrets_reencrypted",
            "encryption_key.removed",
        ] {
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM cms.audit_log WHERE action = $1")
                .bind(action)
                .fetch_one(&before.pool)
                .await
                .unwrap();
            assert!(n >= 1, "{action}");
        }

        // 鍵 v2 だけで起動したサーバーで、保存済みの秘密を使える（INV-KEY-03）
        let after = start(cfg.clone()).await;
        forget_used_steps(&after.pool, "mfa@example.test").await;
        let (res, pending) = password_step(&after, "mfa@example.test", PASSWORD).await;
        assert_eq!(res.status, 200);
        let res = mfa_step(
            &after,
            &pending.unwrap(),
            &format!("{{\"totpCode\":\"{}\"}}", code(&secret, 0)),
        )
        .await;
        assert_eq!(res.status, 200, "{}", res.body);
        // 設定の途中の秘密も読める（確かめのコードは誤りなので、復号できたうえでの不一致になる）
        let p = login(&after, "pending@example.test", PASSWORD).await;
        let res = send(
            &after,
            "POST",
            "/api/admin/me/mfa/totp/confirm",
            &p,
            Some("{\"code\":\"000000\"}"),
        )
        .await;
        assert!(
            res.status == 400 || res.status == 422,
            "{} {}",
            res.status,
            res.body
        );
        // ジョブの中身も読める
        let delivered = smtp.lock().unwrap().received.len();
        assert!(worker::run_once(&after.state).await.unwrap());
        let failed: i64 =
            sqlx::query_scalar("SELECT count(*) FROM cms.jobs WHERE last_error IS NOT NULL")
                .fetch_one(&after.pool)
                .await
                .unwrap();
        assert_eq!(failed, 0);
        assert_eq!(smtp.lock().unwrap().received.len(), delivered + 1);

        let _ = std::fs::remove_dir_all(&dir);
    })
    .await;
}
