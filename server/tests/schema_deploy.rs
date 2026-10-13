//! テンプレートのデプロイの結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `CMS_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use cms_server::workspace_db::{self, WorkspaceDb};
use common::server::*;
use common::with_scratch;
use uuid::Uuid;

fn manifest(extra_fields: &str, subtitle_required: bool, locales: &str) -> String {
    format!(
        r#"{{
          "locales": [{locales}],
          "defaultLocale": "ja",
          "body": {{
            "nodes": [
              {{"name": "doc", "children": ["paragraph"]}},
              {{"name": "paragraph", "children": ["text"], "marks": true}},
              {{"name": "text"}}
            ],
            "marks": [{{"name": "bold"}}],
            "embedProviders": ["www.youtube.com"]
          }},
          "contentTypes": [{{
            "name": "post",
            "urlTemplate": "https://site.example.test/{{locale}}/blog/{{slug}}",
            "listLimit": 5,
            "description": "DESC-MARKER",
            "devNote": "DEVNOTE-MARKER",
            "fields": [
              {{"name": "title", "type": "text", "required": true}},
              {{"name": "slug", "type": "slug", "required": true}},
              {{"name": "subtitle", "type": "text", "required": {subtitle_required}}},
              {{"name": "summary", "type": "text", "description": "DESC-MARKER", "devNote": "DEVNOTE-MARKER"}},
              {{"name": "body", "type": "richtext"}},
              {{"name": "related", "type": "reference", "referenceTo": "post", "multiple": true}}
              {extra_fields}
            ]
          }}]
        }}"#
    )
}

fn base() -> String {
    manifest("", false, r#""ja", "en""#)
}

fn body(manifest: &str, flags: &str) -> String {
    format!("{{\"manifest\":{manifest}{flags}}}")
}

async fn deploy(server: &Server, token: &str, template: &str, body: &str) -> Resp {
    request(
        server,
        "POST",
        &format!("/api/admin/templates/{template}/deploy"),
        &[("Authorization", &format!("Bearer {token}"))],
        Some(body),
    )
    .await
}

async fn columns(server: &Server, ws: Uuid) -> Vec<String> {
    let db = WorkspaceDb::names_for(ws);
    sqlx::query_scalar(
        "SELECT column_name::text FROM information_schema.columns
         WHERE table_schema = $1 AND table_name = 'ct_post' ORDER BY ordinal_position",
    )
    .bind(db.schema.as_str())
    .fetch_all(&server.state.migration)
    .await
    .unwrap()
}

/// 公開中の記事を1件、ワークスペースのロールで直接作る（記事のAPIは次の段階で作る）。
async fn published_entry(server: &Server, ws: Uuid, locale: &str, fields: &str) -> Uuid {
    let db = workspace_db::lookup(&server.pool, ws)
        .await
        .unwrap()
        .unwrap();
    let mut tx = workspace_db::begin(&server.pool, &db).await.unwrap();
    let (entry, version) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("INSERT INTO entries (id, content_type, locale, translation_group_id) VALUES ($1, 'post', $2, $1)")
        .bind(entry)
        .bind(locale)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO ct_post (entry_id) VALUES ($1)")
        .bind(entry)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO versions (id, entry_id, number, schema_version, fields) VALUES ($1, $2, 1, 1, $3::jsonb)")
        .bind(version)
        .bind(entry)
        .bind(fields)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE entries SET published_version_id = $1 WHERE id = $2")
        .bind(version)
        .bind(entry)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    entry
}

#[tokio::test]
async fn deploying_templates() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        create_user(&server.pool, "root@example.test", true).await;
        let root = login(&server, "root@example.test", PASSWORD).await;
        enroll(&server, &root).await;
        let res = send(&server, "POST", "/api/admin/me/tokens", &root, Some("{\"name\":\"deploy\",\"scopes\":[\"deploy\"],\"workspaceIds\":[]}")).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let token = res.json_str("token").unwrap();
        let res = send(&server, "POST", "/api/admin/me/tokens", &root, Some("{\"name\":\"rw\",\"scopes\":[\"read\",\"write\"],\"workspaceIds\":[]}")).await;
        let rw = res.json_str("token").unwrap();
        let t = "blog";

        // 認証: トークンがない・Cookieのセッション・デプロイの範囲がない → 拒否
        let res = request(&server, "POST", &format!("/api/admin/templates/{t}/deploy"), &[], Some(&body(&base(), ""))).await;
        assert_eq!(res.status, 401);
        let res = send(&server, "POST", &format!("/api/admin/templates/{t}/deploy"), &root, Some(&body(&base(), ""))).await;
        assert_eq!(res.status, 401, "{}", res.body);
        assert_eq!(deploy(&server, &rw, t, &body(&base(), "")).await.status, 403);
        assert_eq!(deploy(&server, &token, "Bad_Name", &body(&base(), "")).await.status, 400);

        // 名前の検査（INV-SCH-06）。CLIを通さずに呼んでも拒否する
        for bad in ["Post", "post-name", "post name", "post\\\";drop table x;--", "ポスト", "post$", "", "addedField"] {
            let m = base().replace("\"name\": \"subtitle\"", &format!("\"name\": \"{bad}\""));
            let res = deploy(&server, &token, t, &body(&m, "")).await;
            assert!(res.status == 422 || res.status == 400, "{bad}: {} {}", res.status, res.body);
            let m = base().replace("\"name\": \"post\"", &format!("\"name\": \"{bad}\""));
            let res = deploy(&server, &token, t, &body(&m, "")).await;
            assert!(res.status == 422 || res.status == 400, "{bad}: {} {}", res.status, res.body);
        }
        assert_eq!(deploy(&server, &token, t, "{\"manifest\":{},\"sql\":1}").await.status, 400);

        // 最初のデプロイ
        let res = deploy(&server, &token, t, &body(&base(), "")).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert!(res.body.contains("\"version\":1") && res.body.contains("\"workspaces\":[]"), "{}", res.body);

        // テンプレートを使うワークスペースを作ると、最新の版が適用される
        let mk = |name: &'static str| {
            let server = &server;
            let root = &root;
            async move {
                let res = send(server, "POST", "/api/admin/workspaces", root, Some(&format!("{{\"name\":\"{name}\",\"template\":\"blog\"}}"))).await;
                assert_eq!(res.status, 201, "{}", res.body);
                assert!(res.body.contains("\"schemaVersion\":1") && res.body.contains("\"schemaState\":\"ok\""), "{}", res.body);
                res.json_str("id").unwrap().parse::<Uuid>().unwrap()
            }
        };
        let a = mk("A").await;
        let b = mk("B").await;
        assert_eq!(columns(&server, a).await, ["entry_id", "f_title", "f_slug", "f_subtitle", "f_summary", "f_body", "f_related"]);
        // テンプレートのないワークスペースは、まだ何も適用されていない
        let res = send(&server, "POST", "/api/admin/workspaces", &root, Some("{\"name\":\"C\"}")).await;
        assert!(res.body.contains("\"schemaVersion\":null") && res.body.contains("\"schemaState\":\"none\""), "{}", res.body);

        // スキーマ情報: description は出し、devNote は出さない（INV-SCH-01）
        let info = get(&server, &format!("/api/admin/workspaces/{a}/schema"), &root).await;
        assert_eq!(info.status, 200, "{}", info.body);
        assert!(info.body.contains("DESC-MARKER") && !info.body.contains("DEVNOTE-MARKER") && !info.body.contains("devNote"), "{}", info.body);

        // 追加の変更 → 新しい版。同じ内容をもう一度 → 版は増えない
        let v2 = manifest(r#", {"name": "added_field", "type": "text"}"#, false, r#""ja", "en""#);
        let res = deploy(&server, &token, t, &body(&v2, "")).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert!(res.body.contains("\"version\":2") && res.body.matches("\"state\":\"ok\"").count() == 2, "{}", res.body);
        assert!(columns(&server, b).await.contains(&"f_added_field".to_owned()));
        let res = deploy(&server, &token, t, &body(&v2, "")).await;
        assert!(res.body.contains("\"version\":2") && res.body.contains("\"unchanged\":true") && res.body.contains("\"workspaces\":[]"), "{}", res.body);

        // 破壊的な変更は、明示的な指定がなければ拒否（INV-SCH-02）
        for (label, m) in [
            ("field removed", manifest("", false, r#""ja", "en""#)),
            ("type changed", v2.replace(r#"{"name": "added_field", "type": "text"}"#, r#"{"name": "added_field", "type": "richtext"}"#)),
            ("locale removed", manifest(r#", {"name": "added_field", "type": "text"}"#, false, r#""ja""#)),
        ] {
            let res = deploy(&server, &token, t, &body(&m, "")).await;
            assert_eq!(res.status, 409, "{label}: {}", res.body);
            assert!(res.body.contains("destructive_change"));
        }
        assert!(columns(&server, a).await.contains(&"f_added_field".to_owned()));

        // 一方のワークスペースだけ移行に失敗する（INV-SCH-04, 05）: A には副題のない公開中の記事、B の記事には副題がある
        published_entry(&server, a, "en", r#"{"title": "t", "slug": "s"}"#).await;
        let fixme = published_entry(&server, a, "ja", r#"{"title": "t", "slug": "s2", "subtitle": ""}"#).await;
        published_entry(&server, b, "ja", r#"{"title": "t", "slug": "s", "subtitle": "x"}"#).await;
        let v3 = manifest(r#", {"name": "added_field", "type": "text"}, {"name": "extra_field_v3", "type": "text"}"#, true, r#""ja", "en""#);
        let res = deploy(&server, &token, t, &body(&v3, ",\"allowDestructive\":true")).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert!(res.body.contains(&format!("{{\"workspaceId\":\"{a}\",\"state\":\"pending\",\"error\":\"required_value_missing:post.subtitle\"}}")), "{}", res.body);
        assert!(res.body.contains(&format!("{{\"workspaceId\":\"{b}\",\"state\":\"ok\"}}")), "{}", res.body);
        // A は前の版のまま（DDLも巻き戻っている）、B は新しい版
        assert!(!columns(&server, a).await.contains(&"f_extra_field_v3".to_owned()));
        assert!(columns(&server, b).await.contains(&"f_extra_field_v3".to_owned()));
        let info = get(&server, &format!("/api/admin/workspaces/{a}/schema"), &root).await;
        assert!(info.body.contains("\"version\":2") && info.body.contains("\"state\":\"pending\"") && info.body.contains("\"targetVersion\":3") && !info.body.contains("extra_field_v3"), "{}", info.body);
        assert!(get(&server, &format!("/api/admin/workspaces/{b}/schema"), &root).await.body.contains("extra_field_v3"));

        // データを直してから同じ内容をデプロイすると、移行待ちのワークスペースだけやり直す
        let db = workspace_db::lookup(&server.pool, a).await.unwrap().unwrap();
        let mut tx = workspace_db::begin(&server.pool, &db).await.unwrap();
        sqlx::query("UPDATE versions SET fields = fields || '{\"subtitle\": \"fixed\"}'::jsonb").execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        let _ = fixme;
        let res = deploy(&server, &token, t, &body(&v3, ",\"allowDestructive\":true")).await;
        assert!(res.body.contains("\"unchanged\":true") && res.body.contains(&format!("{{\"workspaceId\":\"{a}\",\"state\":\"ok\"}}")), "{}", res.body);
        assert!(columns(&server, a).await.contains(&"f_extra_field_v3".to_owned()));

        // 使っているロケールは外せない（明示的に許しても、そのワークスペースは移行待ち）
        let v4 = manifest(r#", {"name": "added_field", "type": "text"}, {"name": "extra_field_v3", "type": "text"}"#, true, r#""ja""#);
        let res = deploy(&server, &token, t, &body(&v4, ",\"allowDestructive\":true")).await;
        assert!(res.body.contains(&format!("{{\"workspaceId\":\"{a}\",\"state\":\"pending\",\"error\":\"locale_in_use:en\"}}")), "{}", res.body);
        assert!(res.body.contains(&format!("{{\"workspaceId\":\"{b}\",\"state\":\"ok\"}}")), "{}", res.body);

        // 明示的に許したフィールドの削除・型の変更
        let v5 = manifest(r#", {"name": "extra_field_v3", "type": "richtext"}"#, true, r#""ja""#);
        let res = deploy(&server, &token, t, &body(&v5, ",\"allowDestructive\":true")).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let cols = columns(&server, b).await;
        assert!(!cols.contains(&"f_added_field".to_owned()) && cols.contains(&"f_extra_field_v3".to_owned()), "{cols:?}");
        let ty: String = sqlx::query_scalar("SELECT data_type::text FROM information_schema.columns WHERE table_schema = $1 AND table_name = 'ct_post' AND column_name = 'f_extra_field_v3'")
            .bind(WorkspaceDb::names_for(b).schema.as_str())
            .fetch_one(&server.state.migration)
            .await
            .unwrap();
        assert_eq!(ty, "jsonb");

        // 監査ログ
        for (action, ws) in [("template.deployed", None), ("schema.migrated", Some(b)), ("schema.migration_failed", Some(a))] {
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM cms.audit_log WHERE action = $1 AND workspace_id IS NOT DISTINCT FROM $2")
                .bind(action)
                .bind(ws)
                .fetch_one(&server.pool)
                .await
                .unwrap();
            assert!(n >= 1, "{action}");
        }

        // 本文で使っているマークは外せない（そのワークスペースは移行待ち）。使っていない側は移行する
        let res = send(&server, "POST", "/api/admin/workspaces", &root, Some("{\"name\":\"D\",\"template\":\"blog\"}")).await;
        assert!(res.body.contains("\"schemaVersion\":5"), "{}", res.body);
        let c: Uuid = res.json_str("id").unwrap().parse().unwrap();
        published_entry(&server, b, "ja", r#"{"title": "t", "slug": "s9", "subtitle": "x", "body": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "x", "marks": [{"type": "bold"}]}]}]}}"#).await;
        let v6 = v5.replace(r#""marks": [{"name": "bold"}]"#, r#""marks": []"#);
        let res = deploy(&server, &token, t, &body(&v6, ",\"allowDestructive\":true")).await;
        assert!(res.body.contains(&format!("{{\"workspaceId\":\"{b}\",\"state\":\"pending\",\"error\":\"body_node_in_use:bold\"}}")), "{}", res.body);
        assert!(res.body.contains(&format!("{{\"workspaceId\":\"{c}\",\"state\":\"ok\"}}")), "{}", res.body);

        // サーバー管理者でなくなったら、デプロイの範囲のトークンも使えない
        sqlx::query("UPDATE cms.users SET is_server_admin = false WHERE email = 'root@example.test'").execute(&server.pool).await.unwrap();
        assert_eq!(deploy(&server, &token, t, &body(&v5, "")).await.status, 403);
    })
    .await;
}
