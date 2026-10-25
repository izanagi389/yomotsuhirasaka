//! DBのロールと権限の結合テスト（本物のPostgreSQLを使う）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` にスーパーユーザーの接続先を渡すと実行する（なければ飛ばす）。
//! テストごとに使い捨てのロールとDBを作り、終わったら消す。
//!
//! ここで確かめるのは実装の部品の振る舞い。約束そのもの（INV-DB-*、INV-CFG-03）は
//! tests/invariants/ が外から確かめる。

mod common;

use cms_server::{db, migrate, workspace_db};
use common::{Scratch, superuser_url};
use sqlx::Executor;
use uuid::Uuid;

/// SQLが権限の不足（42501）で失敗することを確かめる。
async fn expect_denied<'c, E: Executor<'c, Database = sqlx::Postgres>>(
    executor: E,
    sql: &str,
    what: &str,
) {
    let err = executor
        .execute(db::trusted(sql.to_owned()))
        .await
        .err()
        .unwrap_or_else(|| panic!("{what}: should be denied but succeeded: {sql}"));
    let code = err
        .as_database_error()
        .and_then(|e| e.code())
        .map(|c| c.into_owned());
    assert_eq!(
        code.as_deref(),
        Some("42501"),
        "{what}: expected permission denied, got {err}"
    );
}

#[tokio::test]
async fn workspace_roles_are_isolated() {
    let Some(url) = superuser_url() else {
        eprintln!("skipped: CMS_TEST_PG_SUPERUSER_URL is not set");
        return;
    };
    let s = Scratch::new(&url).await;
    let (app_cfg, migration_cfg) = (s.app.clone(), s.migration.clone());
    let result = async move {
        let migration = db::migration_pool(&migration_cfg).await.unwrap();
        let app = db::app_pool(&app_cfg).await.unwrap();
        let app_role = db::current_role(&app).await.unwrap();

        // 移行は2回目には何もしない
        assert_eq!(
            migrate::run(&migration).await.unwrap(),
            migrate::MIGRATIONS.iter().map(|m| m.0).collect::<Vec<_>>()
        );
        assert_eq!(migrate::run(&migration).await.unwrap(), Vec::<i32>::new());
        migrate::apply_grants(&migration, &app_role).await.unwrap();
        migrate::apply_grants(&migration, &app_role).await.unwrap();

        let (a_id, b_id) = (Uuid::new_v4(), Uuid::new_v4());
        let a = workspace_db::provision(&migration, &app_role, a_id, "A", "inv-base")
            .await
            .unwrap();
        let b = workspace_db::provision(&migration, &app_role, b_id, "B", "inv-base")
            .await
            .unwrap();
        // ワークスペースのテーブル（移行の役目のロールが作る）
        for w in [&a, &b] {
            migration
                .execute(cms_server::sql!(
                    "CREATE TABLE {}.posts (id int PRIMARY KEY, title text)",
                    w.schema
                ))
                .await
                .unwrap();
        }

        // 登録から引ける
        let found = workspace_db::lookup(&app, a_id).await.unwrap().unwrap();
        assert_eq!(found.schema, a.schema);
        assert!(
            workspace_db::lookup(&app, Uuid::new_v4())
                .await
                .unwrap()
                .is_none()
        );

        // Aのロールに切り替えると、Aのテーブルだけを読み書きできる
        {
            let mut tx = workspace_db::begin(&app, &a).await.unwrap();
            tx.execute("INSERT INTO posts (id, title) VALUES (1, 'a')")
                .await
                .unwrap();
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM posts")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            assert_eq!(n, 1);
            tx.rollback().await.unwrap();
        }
        let b_table = format!("{}.posts", b.schema);
        for (sql, what) in [
            (format!("SELECT * FROM {b_table}"), "read another workspace"),
            (
                format!("INSERT INTO {b_table} VALUES (9, 'x')"),
                "write another workspace",
            ),
            (
                "SELECT * FROM cms.workspaces".to_owned(),
                "read shared table",
            ),
            (
                "INSERT INTO cms.audit_log (id, action) VALUES (gen_random_uuid(), 'x')".to_owned(),
                "write shared table",
            ),
            ("CREATE TABLE extra (x int)".to_owned(), "DDL in own schema"),
            ("DROP TABLE posts".to_owned(), "drop own table"),
        ] {
            let mut tx = workspace_db::begin(&app, &a).await.unwrap();
            expect_denied(&mut *tx, &sql, what).await;
            tx.rollback().await.unwrap();
        }

        // 普段用のロールのまま（切り替えない）
        let a_table = format!("{}.posts", a.schema);
        for (sql, what) in [
            (
                format!("SELECT * FROM {a_table}"),
                "read workspace data without switching (INHERIT FALSE)",
            ),
            (
                "UPDATE cms.audit_log SET action = action".to_owned(),
                "update audit log",
            ),
            ("DELETE FROM cms.audit_log".to_owned(), "delete audit log"),
            ("TRUNCATE cms.audit_log".to_owned(), "truncate audit log"),
            ("CREATE ROLE t_should_not_exist".to_owned(), "create role"),
            (
                "CREATE SCHEMA t_should_not_exist".to_owned(),
                "create schema",
            ),
            (
                "CREATE TABLE public.t_should_not_exist (x int)".to_owned(),
                "create table in public",
            ),
            (
                "CREATE TABLE cms.t_should_not_exist (x int)".to_owned(),
                "create table in cms",
            ),
            (
                "UPDATE cms.workspaces SET name = name".to_owned(),
                "update workspace registry",
            ),
        ] {
            expect_denied(&app, &sql, what).await;
        }
        // 監査ログへの追記と読み取りはできる
        app.execute("INSERT INTO cms.audit_log (id, action) VALUES (gen_random_uuid(), 'test')")
            .await
            .unwrap();
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM cms.audit_log")
            .fetch_one(&app)
            .await
            .unwrap();
        assert_eq!(n, 1);
    };
    // 失敗しても後片付けをしてから、失敗を伝える
    let outcome = tokio::spawn(result).await;
    s.drop().await;
    if let Err(e) = outcome {
        std::panic::resume_unwind(e.into_panic());
    }
}

#[tokio::test]
async fn grants_refuse_tables_without_policy() {
    let Some(url) = superuser_url() else {
        eprintln!("skipped: CMS_TEST_PG_SUPERUSER_URL is not set");
        return;
    };
    let s = Scratch::new(&url).await;
    let migration = db::migration_pool(&s.migration).await.unwrap();
    let app = db::app_pool(&s.app).await.unwrap();
    let app_role = db::current_role(&app).await.unwrap();
    migrate::run(&migration).await.unwrap();
    migration
        .execute("CREATE TABLE cms.unlisted (x int)")
        .await
        .unwrap();
    let err = migrate::apply_grants(&migration, &app_role)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unlisted"), "{err}");
    drop((migration, app));
    s.drop().await;
}

#[tokio::test]
async fn audit_log_is_purged_only_through_the_dedicated_function() {
    let Some(url) = superuser_url() else {
        eprintln!("skipped: CMS_TEST_PG_SUPERUSER_URL is not set");
        return;
    };
    let s = Scratch::new(&url).await;
    let (app_cfg, migration_cfg) = (s.app.clone(), s.migration.clone());
    let result = async move {
        let migration = db::migration_pool(&migration_cfg).await.unwrap();
        let app = db::app_pool(&app_cfg).await.unwrap();
        let app_role = db::current_role(&app).await.unwrap();
        migrate::run(&migration).await.unwrap();
        // 2回目も同じ結果になる（起動のたびに作り直す）
        migrate::apply_grants(&migration, &app_role).await.unwrap();
        migrate::apply_grants(&migration, &app_role).await.unwrap();

        // 古い記録（400日前）と新しい記録
        let insert = |days: i32, action: &str| {
            format!(
                "INSERT INTO cms.audit_log (id, action, created_at) VALUES (gen_random_uuid(), '{action}', now() - interval '{days} days')"
            )
        };
        for (days, action) in [(400, "old1"), (400, "old2"), (1, "new")] {
            migration
                .execute(db::trusted(insert(days, action)))
                .await
                .unwrap();
        }
        let actions = || async {
            let mut v: Vec<String> =
                sqlx::query_scalar("SELECT action FROM cms.audit_log ORDER BY action")
                    .fetch_all(&app)
                    .await
                    .unwrap();
            v.sort();
            v
        };

        // 普段用のロールは、行を直接は消せない。30日より短い指定は受け付けない
        expect_denied(
            &app,
            "DELETE FROM cms.audit_log",
            "delete audit log directly",
        )
        .await;
        assert!(
            sqlx::query_scalar::<_, i64>("SELECT cms.purge_audit_log(10)")
                .fetch_one(&app)
                .await
                .is_err()
        );

        // 送り終えていないインスタンス全体の書き出し先があれば、その記録は消さない
        migration
            .execute("INSERT INTO cms.audit_export_cursors (sink_key, cursor_at) VALUES ('stdout', now() - interval '500 days')")
            .await
            .unwrap();
        let n: i64 = sqlx::query_scalar("SELECT cms.purge_audit_log(365)")
            .fetch_one(&app)
            .await
            .unwrap();
        assert_eq!(n, 0);
        // 停止した書き出し先は止めない
        migration
            .execute("UPDATE cms.audit_export_cursors SET stopped_at = now()")
            .await
            .unwrap();
        let n: i64 = sqlx::query_scalar("SELECT cms.purge_audit_log(365)")
            .fetch_one(&app)
            .await
            .unwrap();
        assert_eq!(n, 2);
        assert_eq!(actions().await, vec!["new".to_owned()]);

        // 専用のロールは、ログインできず、普段用のロールはそのロールになれない
        let purger = migrate::audit_purger_role(&app_role).unwrap();
        let can_login: bool =
            sqlx::query_scalar("SELECT rolcanlogin FROM pg_roles WHERE rolname = $1")
                .bind(purger.as_str())
                .fetch_one(&migration)
                .await
                .unwrap();
        assert!(!can_login);
        expect_denied(
            &app,
            &format!("SET ROLE {purger}"),
            "become the purger role",
        )
        .await;
    };
    let outcome = tokio::spawn(result).await;
    s.drop().await;
    if let Err(e) = outcome {
        std::panic::resume_unwind(e.into_panic());
    }
}
