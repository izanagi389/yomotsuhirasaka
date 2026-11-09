//! 監査ログの書き出し先の停止と再開（13-security.md「管理者への通知」、11-permissions-and-audit.md「外部への書き出し」）。
//!
//! - 7日間続けて送れないと止まり、そのことが監査ログ（管理者への通知）に1回だけ残る
//! - 設定ファイルの書き出し先は、サーバー上のコマンドの処理で続きから再開できる。今の設定にないものは再開しない
//! - 設定ファイルから外した書き出し先は、保存期間を過ぎた記録の削除を止めない。戻すと続きから送る
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::content::*;
use common::server::*;
use common::with_scratch;
use sqlx::PgPool;
use uuid::Uuid;
use yomi_server::audit;
use yomi_server::audit_export::{new_secret, register_instance_sinks, resume_instance_sinks, tick};
use yomi_server::config::AuditExportConfig;
use yomi_server::db;

async fn stopped_records(pool: &PgPool, workspace: Option<Uuid>) -> (i64, i64) {
    sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE notify) FROM yomi.audit_log
         WHERE action = 'audit_export.stopped' AND workspace_id IS NOT DISTINCT FROM $1",
    )
    .bind(workspace)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn is_stopped(pool: &PgPool, key: &str) -> bool {
    sqlx::query_scalar(
        "SELECT stopped_at IS NOT NULL FROM yomi.audit_export_cursors WHERE sink_key = $1",
    )
    .bind(key)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn stops_notifies_and_resumes() {
    with_scratch(|app, migration| async move {
        // 書き出し先のファイルの場所にディレクトリを置いて、書けないようにする
        let dir = std::env::temp_dir().join(format!("yomi-audit-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("audit.jsonl");
        std::fs::create_dir(&path).unwrap();
        let mut cfg = config(&app, &migration);
        cfg.audit_export.file = Some(path.clone());
        let export = cfg.audit_export.clone();
        let server = start(cfg).await;
        let pool = &server.pool;
        register_instance_sinks(pool, &export).await.unwrap();

        // 7日より前から失敗し続けている。今の設定にない書き出し先も止まっている
        let key = format!("file:{}", path.display());
        sqlx::query(
            "UPDATE yomi.audit_export_cursors
             SET cursor_at = now() - interval '1 hour', failing_since = now() - interval '8 days',
                 last_attempt_at = now() - interval '1 hour', last_error = 'file_open'
             WHERE sink_key = $1",
        )
        .bind(&key)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO yomi.audit_export_cursors (sink_key, stopped_at) VALUES ('file:/gone', now())")
            .execute(pool)
            .await
            .unwrap();

        // ワークスペースの書き出し先（宛先の名前を解決できないので失敗する）
        let (root, _) = root_with_deploy_token(&server).await;
        let res = send(&server, "POST", "/api/admin/workspaces", &root, Some(r#"{"name":"A"}"#)).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let ws: Uuid = res.json_str("id").unwrap().parse().unwrap();
        let sink = Uuid::new_v4();
        let (_, sealed) = new_secret(&server.state.keys, sink).unwrap();
        sqlx::query(
            "INSERT INTO yomi.audit_sinks (id, workspace_id, url, secret_enc, cursor_at, failing_since, last_attempt_at)
             VALUES ($1, $2, 'https://logs.example.test/x', $3, now() - interval '1 hour', now() - interval '8 days', now() - interval '1 hour')",
        )
        .bind(sink)
        .bind(ws)
        .bind(sealed)
        .execute(pool)
        .await
        .unwrap();

        // 送る記録を作る（5秒より前の記録だけを送る）
        audit::record(pool, Some(ws), None, audit::action::WORKSPACE_CREATED, &[])
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(6)).await;

        tick(&server.state).await.unwrap();
        assert!(is_stopped(pool, &key).await);
        assert_eq!(stopped_records(pool, None).await, (1, 1));
        assert_eq!(stopped_records(pool, Some(ws)).await, (1, 1));

        // 止まった後は、もう試さず、通知も増えない
        tick(&server.state).await.unwrap();
        assert_eq!(stopped_records(pool, None).await, (1, 1));
        assert_eq!(stopped_records(pool, Some(ws)).await, (1, 1));

        // 送り先を直して再開すると、続きから送る。今の設定にないものは再開しない
        std::fs::remove_dir(&path).unwrap();
        let resumed = resume_instance_sinks(pool, &export).await.unwrap();
        assert_eq!(resumed, vec![key.clone()]);
        assert!(!is_stopped(pool, &key).await);
        assert!(is_stopped(pool, "file:/gone").await);
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM yomi.audit_log WHERE action = 'audit_export.resumed'")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
        tick(&server.state).await.unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        // 止まる前に送れなかった記録が、続きから届く
        assert!(written.contains("workspace.created"), "{written}");

        // 止まっているものがなければ何もしない
        assert!(resume_instance_sinks(pool, &export).await.unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    })
    .await;
}

async fn count_action(pool: &PgPool, action: &str) -> (i64, i64) {
    sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE notify) FROM yomi.audit_log WHERE action = $1",
    )
    .bind(action)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn purge(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT yomi.purge_audit_log()")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn removed_sink_does_not_block_purge() {
    with_scratch(|app, migration| async move {
        let dir = std::env::temp_dir().join(format!("yomi-audit-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("audit.jsonl");
        let mut cfg = config(&app, &migration);
        cfg.audit_export.file = Some(path.clone());
        let export = cfg.audit_export.clone();
        let server = start(cfg).await;
        let pool = &server.pool;
        let strong = db::migration_pool(&migration).await.unwrap();
        register_instance_sinks(pool, &export).await.unwrap();

        // 長く送れていない書き出し先と、保存期間を過ぎた記録
        let key = format!("file:{}", path.display());
        sqlx::query("UPDATE yomi.audit_export_cursors SET cursor_at = now() - interval '500 days' WHERE sink_key = $1")
            .bind(&key)
            .execute(pool)
            .await
            .unwrap();
        let old_record = |action: &'static str, days: i32| {
            let strong = strong.clone();
            async move {
                sqlx::query("INSERT INTO yomi.audit_log (id, action, created_at) VALUES (gen_random_uuid(), $1, now() - make_interval(days => $2))")
                    .bind(action)
                    .bind(days)
                    .execute(&strong)
                    .await
                    .unwrap();
            }
        };
        old_record("old.first", 400).await;

        // 設定にある間は、送り終えるまで消さない
        assert_eq!(purge(pool).await, 0);

        // 設定から外すと止まり、削除を止めない。止めたことは1回だけ知らせる
        let none = AuditExportConfig::default();
        register_instance_sinks(pool, &none).await.unwrap();
        assert!(is_stopped(pool, &key).await);
        assert_eq!(count_action(pool, "audit_export.removed").await, (1, 1));
        assert_eq!(purge(pool).await, 1);
        register_instance_sinks(pool, &none).await.unwrap();
        assert_eq!(count_action(pool, "audit_export.removed").await, (1, 1));

        // 送れずに止まったものは、設定に戻しても止まったまま
        let failed = dir.join("failed.jsonl");
        let failed_key = format!("file:{}", failed.display());
        sqlx::query("INSERT INTO yomi.audit_export_cursors (sink_key, stopped_at, last_error) VALUES ($1, now(), 'file_open')")
            .bind(&failed_key)
            .execute(pool)
            .await
            .unwrap();
        let failed_export = AuditExportConfig { file: Some(failed), ..Default::default() };
        register_instance_sinks(pool, &failed_export).await.unwrap();
        assert!(is_stopped(pool, &failed_key).await);

        // 外したものを戻すと、次の起動で続きから送る
        old_record("old.second", 1).await;
        register_instance_sinks(pool, &export).await.unwrap();
        assert!(!is_stopped(pool, &key).await);
        assert_eq!(count_action(pool, "audit_export.resumed").await, (1, 0));
        tick(&server.state).await.unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("old.second"), "{written}");
        assert!(!written.contains("old.first"), "{written}");
        std::fs::remove_dir_all(&dir).unwrap();
    })
    .await;
}
