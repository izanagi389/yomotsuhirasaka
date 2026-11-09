//! フックが読む秘密の結合テスト（04-hooks.md「秘密」）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::json;
use yomi_server::hook_secrets;
use yomi_server::script::{Engine, Host, Limits, Permission, Script};
use yomi_server::workspace_db;

const VALUE: &str = "HOOK-SECRET-VALUE-do-not-leak";

#[tokio::test]
async fn secrets_are_write_only_and_reach_only_declared_async_hooks() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        let other = workspace_with_template(&server, &root, "B", "blog").await;
        create_user(&server.pool, "editor@example.test", false).await;
        add_member(&server, ws, "editor@example.test", "editor").await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let base = format!("/api/admin/workspaces/{ws}/hook-secrets");
        let put = |name: &str, value: &str| {
            let (server, root, path) = (&server, &root, format!("{base}/{name}"));
            let body = json!({"value": value}).to_string();
            async move { send(server, "PUT", &path, root, Some(&body)).await }
        };

        // ワークスペースの管理者だけ
        assert_eq!(get(&server, &base, &editor).await.status, 403);
        let res = send(&server, "PUT", &format!("{base}/API_KEY"), &editor, Some(r#"{"value":"x"}"#)).await;
        assert_eq!(res.status, 403);

        // 形の検査
        assert_eq!(put("bad-name", "x").await.status, 422);
        assert_eq!(put("API_KEY", "").await.status, 422);
        assert_eq!(put("API_KEY", &"x".repeat(4097)).await.status, 422);

        // 登録しても値は返らず、DBにも平文で残らない。名前と値は監査ログに書かない
        let res = put("API_KEY", VALUE).await;
        assert_eq!(res.status, 204, "{}", res.body);
        let list = get(&server, &base, &root).await;
        assert_eq!(list.status, 200);
        assert!(list.body.contains("API_KEY"));
        assert!(!list.body.contains(VALUE));
        let sealed: Vec<u8> = sqlx::query_scalar("SELECT value_enc FROM yomi.hook_secrets WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert!(!sealed.windows(VALUE.len()).any(|w| w == VALUE.as_bytes()));
        let audited: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM yomi.audit_log WHERE workspace_id = $1 AND action = 'hook_secret.saved'",
        )
        .bind(ws)
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert_eq!(audited, 1);

        // 宣言した名前だけを復号する。他のワークスペースからは読めない
        let keys = &server.state.keys;
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let loaded = hook_secrets::load(&server.pool, keys, ws, &names(&["API_KEY", "NONE"])).await.unwrap();
        assert_eq!(loaded.get("API_KEY").map(String::as_str), Some(VALUE));
        assert_eq!(loaded.len(), 1);
        assert!(hook_secrets::load(&server.pool, keys, other, &names(&["API_KEY"])).await.unwrap().is_empty());

        // 非同期のフックは読める。同期の関数には `yomi.secrets` がない。権限がなければ読めない
        let engine = Engine::new(Limits {
            memory_bytes: 16 * 1024 * 1024,
            time: std::time::Duration::from_secs(2),
            async_time: std::time::Duration::from_secs(2),
        });
        let db = workspace_db::lookup(&server.pool, ws).await.unwrap().unwrap();
        let host = || Host::new(server.pool.clone(), db.clone()).with_secrets(loaded.clone());
        let script = |source: &str, permissions: Vec<Permission>| Script {
            source: source.to_owned(),
            permissions,
            allowed_hosts: vec![],
            secrets: vec!["API_KEY".to_owned()],
        };
        let reads = format!(
            "export default function () {{ if (yomi.secrets.get('API_KEY') !== '{VALUE}') throw new Error('x'); if (yomi.secrets.get('NONE') !== null) throw new Error('y'); }}"
        );
        assert!(engine.after(&script(&reads, vec![Permission::SecretsRead]), host(), &json!({})).await.is_ok());
        assert!(engine.after(&script(&reads, vec![]), host(), &json!({})).await.is_err());
        let absent = "export default function () { return typeof yomi.secrets === 'undefined' ? null : { errors: [{ message: 'visible' }] }; }";
        let errors = engine
            .before_publish(&script(absent, vec![Permission::SecretsRead]), host(), &json!({}))
            .await
            .unwrap();
        assert!(errors.is_empty(), "{errors:?}");

        // 鍵の入れ替えの後も読める
        let outcome = yomi_server::rekey::reencrypt_all(&server.pool, keys).await.unwrap();
        assert_eq!(outcome.failed, 0);

        // 消すと読めない
        let res = send(&server, "DELETE", &format!("{base}/API_KEY"), &root, None).await;
        assert_eq!(res.status, 204, "{}", res.body);
        assert_eq!(send(&server, "DELETE", &format!("{base}/API_KEY"), &root, None).await.status, 404);
        assert!(hook_secrets::load(&server.pool, keys, ws, &names(&["API_KEY"])).await.unwrap().is_empty());

        // 上限
        for i in 0..hook_secrets::MAX_SECRETS {
            assert_eq!(put(&format!("K{i}"), "x").await.status, 204);
        }
        let res = put("ONE_MORE", "x").await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (409, Some("too_many_hook_secrets")));
        assert_eq!(put("K0", "y").await.status, 204, "replacing stays possible at the limit");
    })
    .await;
}
