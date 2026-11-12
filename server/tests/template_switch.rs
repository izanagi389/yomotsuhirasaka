//! テンプレートの切り替えの結合テスト（02-schema-and-delivery-api.md「テンプレートの切り替え」、INV-SCH-08・09）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::content::*;
use common::server::*;
use common::with_scratch;
use serde_json::{Value, json};
use uuid::Uuid;
use yomi_server::workspace_db::{self, WorkspaceDb};

const NODES: &str = r#""body":{"nodes":[{"name":"doc","children":["paragraph"]},{"name":"paragraph","children":["text"],"marks":true},{"name":"text"}],"marks":[{"name":"bold"}],"embedProviders":[]}"#;

fn manifest(types: &str) -> String {
    format!(r#"{{"locales":["ja"],"defaultLocale":"ja",{NODES},"contentTypes":[{types}]}}"#)
}

fn template_a() -> String {
    manifest(
        r#"{"name":"post","urlTemplate":"https://site.example.test/blog/{slug}","listLimit":5,"fields":[
             {"name":"title","type":"text","required":true},{"name":"slug","type":"slug","required":true},
             {"name":"summary","type":"text"},{"name":"body","type":"richtext"}]},
           {"name":"page","urlTemplate":"https://site.example.test/{slug}","listLimit":5,"fields":[
             {"name":"title","type":"text","required":true},{"name":"slug","type":"slug","required":true}]}"#,
    )
}

fn template_b() -> String {
    manifest(
        r#"{"name":"article","urlTemplate":"https://site.example.test/articles/{slug}","listLimit":5,"fields":[
             {"name":"headline","type":"text","required":true},{"name":"slug","type":"slug","required":true},
             {"name":"body","type":"richtext"}]}"#,
    )
}

fn parse(res: &Resp) -> Value {
    serde_json::from_str(&res.body).unwrap_or_else(|_| panic!("{}", res.body))
}

async fn revision(server: &Server, path: &str, s: &Login) -> i64 {
    parse(&get(server, path, s).await)["revision"]
        .as_i64()
        .unwrap()
}

async fn reauth(server: &Server, s: &Login, email: &str, secret: &str) {
    forget_used_steps(&server.pool, email).await;
    let res = send(
        server,
        "POST",
        "/api/admin/session/reauth",
        s,
        Some(&format!("{{\"totpCode\":\"{}\"}}", code(secret, 0))),
    )
    .await;
    assert!(res.status < 300, "{}", res.body);
}

#[tokio::test]
async fn switching_templates_maps_renames_and_rewrites_past_versions() {
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
        assert_eq!(deploy(&server, &token, "tpl-a", &template_a(), "").await.status, 200);
        assert_eq!(deploy(&server, &token, "tpl-b", &template_b(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "tpl-a").await;
        let entries = format!("/api/admin/workspaces/{ws}/entries");

        // 記事を2回公開し（過去の版を作る）、固定ページも1件作る
        let res = send(
            &server,
            "POST",
            &entries,
            &root,
            Some(r#"{"contentType":"post","fields":{"title":"TITLE-1","slug":"s1","summary":"SUMMARY-1"}}"#),
        )
        .await;
        assert_eq!(res.status, 201, "{}", res.body);
        let id = parse(&res)["id"].as_str().unwrap().to_owned();
        let path = format!("{entries}/{id}");
        let publish = |rev: i64| json!({"revision": rev, "correction": false}).to_string();
        let rev = revision(&server, &path, &root).await;
        let res = send(&server, "POST", &format!("{path}/publish"), &root, Some(&publish(rev))).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let rev = revision(&server, &path, &root).await;
        let save = json!({"revision": rev, "fields": {"title": "TITLE-2", "summary": "SUMMARY-2"}}).to_string();
        assert_eq!(send(&server, "PUT", &format!("{path}/draft"), &root, Some(&save)).await.status, 200);
        let rev = revision(&server, &path, &root).await;
        let res = send(&server, "POST", &format!("{path}/publish"), &root, Some(&publish(rev))).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let res = send(
            &server,
            "POST",
            &entries,
            &root,
            Some(r#"{"contentType":"page","fields":{"title":"PAGE-1","slug":"about"}}"#),
        )
        .await;
        assert_eq!(res.status, 201, "{}", res.body);
        let page_id: Uuid = parse(&res)["id"].as_str().unwrap().parse().unwrap();

        // 選べるテンプレート
        let options = parse(&get(&server, &format!("/api/admin/workspaces/{ws}/template-options"), &root).await);
        let names: Vec<&str> = options.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["tpl-b"]);
        let all = parse(&get(&server, "/api/admin/templates", &root).await);
        let names: Vec<&str> = all.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["blog", "tpl-a", "tpl-b"]);
        assert!(all[0]["version"].is_null() && all[0]["bundled"] == true, "{all}");

        let switch = format!("/api/admin/workspaces/{ws}/template");
        let mapping = r#""mapping":{"contentTypes":{"post":"article"},"fields":{"post.title":"headline"}}"#;

        // 試行: 対応づけと、消えるもの（固定ページと要約）
        let res = send(&server, "POST", &switch, &root, Some(&format!(r#"{{"template":"tpl-b",{mapping},"dryRun":true}}"#))).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let plan = parse(&res)["plan"].clone();
        assert_eq!(plan["destructive"], true, "{plan}");
        assert_eq!(plan["problems"], json!([]));
        assert_eq!(plan["contentTypes"][0]["to"], "article");
        assert_eq!(plan["contentTypes"][1]["to"], Value::Null);
        // 対応づけの誤りは試行でも断る
        let res = send(&server, "POST", &switch, &root, Some(r#"{"template":"tpl-b","mapping":{"contentTypes":{"post":"nope"}},"dryRun":true}"#)).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (422, Some("invalid_mapping")), "{}", res.body);
        // デプロイしていない・今と同じテンプレート
        let res = send(&server, "POST", &switch, &root, Some(r#"{"template":"tpl-none","dryRun":true}"#)).await;
        assert_eq!(res.json_str("error").as_deref(), Some("template_not_deployed"), "{}", res.body);
        let res = send(&server, "POST", &switch, &root, Some(r#"{"template":"tpl-a","dryRun":true}"#)).await;
        assert_eq!(res.json_str("error").as_deref(), Some("same_template"), "{}", res.body);

        // 書き出しと切り替えは再認証が要る
        expire_reauth(&server.pool, "root@example.test").await;
        let export = format!("{switch}/export");
        let res = send(&server, "POST", &export, &root, Some(&format!(r#"{{"template":"tpl-b",{mapping}}}"#))).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (403, Some("reauth_required")), "{}", res.body);
        let res = send(&server, "POST", &switch, &root, Some(&format!(r#"{{"template":"tpl-b",{mapping},"allowDestructive":true}}"#))).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (403, Some("reauth_required")), "{}", res.body);
        reauth(&server, &root, "root@example.test", &secret).await;

        // 消えるデータの書き出し: 下書きと過去の版の要約、固定ページ
        let res = send(&server, "POST", &export, &root, Some(&format!(r#"{{"template":"tpl-b",{mapping}}}"#))).await;
        assert_eq!(res.status, 200, "{}", res.body);
        for marker in ["SUMMARY-1", "SUMMARY-2", "PAGE-1"] {
            assert!(res.body.contains(marker), "{marker}: {}", res.body);
        }
        assert!(!res.body.contains("TITLE-2"), "引き継ぐ値は書き出さない: {}", res.body);

        // 明示的な指定がなければ切り替えない
        let res = send(&server, "POST", &switch, &root, Some(&format!(r#"{{"template":"tpl-b",{mapping}}}"#))).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (409, Some("destructive_change")), "{}", res.body);

        // 個人トークンでは切り替えない（権限を付与された管理者のトークンでも）
        create_user(&server.pool, "admin@example.test", false).await;
        add_member(&server, ws, "admin@example.test", "admin").await;
        sqlx::query("UPDATE yomi.memberships SET grants = ARRAY['switchTemplate'] WHERE workspace_id = $1 AND user_id = (SELECT id FROM yomi.users WHERE email = 'admin@example.test')")
            .bind(ws)
            .execute(&server.pool)
            .await
            .unwrap();
        let admin = login(&server, "admin@example.test", PASSWORD).await;
        enroll(&server, &admin).await;
        let res = send(&server, "POST", "/api/admin/me/tokens", &admin, Some(&format!(r#"{{"name":"rw","scopes":["read","write"],"workspaceIds":["{ws}"]}}"#))).await;
        let rw = res.json_str("token").unwrap_or_else(|| panic!("{}", res.body));
        let res = request(&server, "POST", &switch, &[("Authorization", &format!("Bearer {rw}"))], Some(r#"{"template":"tpl-b","dryRun":true}"#)).await;
        assert!(matches!(res.status, 401 | 403), "{}", res.body);

        let res = send(&server, "POST", &switch, &root, Some(&format!(r#"{{"template":"tpl-b",{mapping},"allowDestructive":true}}"#))).await;
        assert_eq!(res.status, 200, "{}", res.body);

        // 表と列の名前、記事のコンテンツタイプ、過去の版の本文が新しい形になる
        let db = WorkspaceDb::names_for(ws);
        let schema = db.schema.as_str();
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT column_name::text FROM information_schema.columns WHERE table_schema = $1 AND table_name = 'ct_article' ORDER BY column_name",
        )
        .bind(schema)
        .fetch_all(&server.state.migration)
        .await
        .unwrap();
        assert_eq!(columns, vec!["entry_id", "f_body", "f_headline", "f_slug"]);
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT table_name::text FROM information_schema.tables WHERE table_schema = $1 AND table_name LIKE 'ct\\_%' ORDER BY 1",
        )
        .bind(schema)
        .fetch_all(&server.state.migration)
        .await
        .unwrap();
        assert_eq!(tables, vec!["ct_article"]);
        let wdb = workspace_db::lookup(&server.pool, ws).await.unwrap().unwrap();
        let mut tx = workspace_db::begin(&server.pool, &wdb).await.unwrap();
        let versions: Vec<(i32, String)> =
            sqlx::query_as("SELECT number, fields::text FROM versions WHERE entry_id = $1 ORDER BY number")
                .bind(Uuid::parse_str(&id).unwrap())
                .fetch_all(&mut *tx)
                .await
                .unwrap();
        let versions: Vec<(i32, Value)> = versions.into_iter().map(|(n, f)| (n, serde_json::from_str(&f).unwrap())).collect();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].1["headline"], "TITLE-1");
        assert_eq!(versions[1].1["headline"], "TITLE-2");
        for (_, f) in &versions {
            assert!(f.get("summary").is_none() && f.get("title").is_none(), "{f}");
        }
        let entry = parse(&get(&server, &path, &root).await);
        assert_eq!(entry["contentType"], "article", "{entry}");
        assert_eq!(entry["fields"]["headline"], "TITLE-2", "{entry}");
        let page_deleted: bool =
            sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM entries WHERE id = $1")
                .bind(page_id)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        drop(tx);
        assert!(page_deleted);
        let (template, state): (String, String) =
            sqlx::query_as("SELECT template, schema_state FROM yomi.workspaces WHERE id = $1")
                .bind(ws)
                .fetch_one(&server.pool)
                .await
                .unwrap();
        assert_eq!((template.as_str(), state.as_str()), ("tpl-b", "ok"));
        // 監査ログに残り、管理者に知らせる
        let notify: Vec<bool> = sqlx::query_scalar(
            "SELECT notify FROM yomi.audit_log WHERE workspace_id = $1 AND action = 'template.switched'",
        )
        .bind(ws)
        .fetch_all(&server.pool)
        .await
        .unwrap();
        assert_eq!(notify, vec![true]);

        // 戻す: 必須の「タイトル」を見出しから引き継ぐ。固定ページは増えるだけ
        let res = send(
            &server,
            "POST",
            &switch,
            &root,
            Some(r#"{"template":"tpl-a","mapping":{"contentTypes":{"article":"post"},"fields":{"article.headline":"title"}}}"#),
        )
        .await;
        assert_eq!(res.status, 200, "{}", res.body);
        let entry = parse(&get(&server, &path, &root).await);
        assert_eq!(entry["fields"]["title"], "TITLE-2", "{entry}");
    })
    .await;
}
