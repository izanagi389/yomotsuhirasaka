//! 版の本文の抹消の結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use std::io::Cursor;

use common::content::*;
use common::server::*;
use common::with_scratch;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use serde_json::{Value, json};

fn parse(res: &Resp) -> Value {
    serde_json::from_str(&res.body).unwrap_or_else(|_| panic!("{}", res.body))
}

fn png(seed: u8) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(RgbImage::from_pixel(4, 4, Rgb([seed, 0, 0])))
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn img(id: &str) -> Value {
    json!({"type": "image", "attrs": {"mediaId": id, "alt": "図"}})
}

fn doc(text: &str, images: &[&str]) -> Value {
    let mut content = vec![
        json!({"type": "heading", "attrs": {"level": 2, "id": "sec"}, "content": [{"type": "text", "text": "見出し"}]}),
        json!({"type": "paragraph", "content": [{"type": "text", "text": text}]}),
    ];
    content.extend(images.iter().map(|i| img(i)));
    json!({"type": "doc", "content": content})
}

async fn fetch_status(server: &Server, url: &str) -> u16 {
    let path = url.strip_prefix("https://yomi.example.test").unwrap();
    request_bytes(server, "GET", path, &[], None).await.status
}

#[tokio::test]
async fn erasing_version_bodies() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        for (email, role) in [("admin@example.test", "admin"), ("editor@example.test", "editor")] {
            create_user(&server.pool, email, false).await;
            add_member(&server, ws, email, role).await;
        }
        let admin = login(&server, "admin@example.test", PASSWORD).await;
        let (totp, _) = enroll(&server, &admin).await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let entries = format!("/api/admin/workspaces/{ws}/entries");
        let up = |seed: u8| {
            let (server, editor, path) = (&server, &editor, format!("/api/admin/workspaces/{ws}/media"));
            async move {
                let res = upload(server, &path, editor, "image/png", &png(seed)).await;
                assert_eq!(res.status, 201, "{}", res.body);
                let v = parse(&res);
                (v["id"].as_str().unwrap().to_owned(), v["url"].as_str().unwrap().to_owned())
            }
        };
        let (only_here, only_here_url) = up(1).await; // 抹消する版だけで使う
        let (in_other_draft, in_other_draft_url) = up(2).await; // 別の記事の下書きでも使う
        let (in_current, _) = up(3).await; // この記事の公開中の版でも使う
        let (in_other_old, _) = up(4).await; // 別の記事の古い版でも使う

        let create = |slug: &str, body: Value| json!({"contentType": "post", "fields": {"title": "t", "slug": slug, "body": body}}).to_string();
        let publish = |path: String, rev: i64| {
            let (server, editor) = (&server, &editor);
            async move {
                let res = send(server, "POST", &format!("{path}/publish"), editor, Some(&json!({"revision": rev}).to_string())).await;
                assert_eq!(res.status, 200, "{}", res.body);
                parse(&res)["versionId"].as_str().unwrap().to_owned()
            }
        };
        let save = |path: String, rev: i64, body: Value| {
            let (server, editor) = (&server, &editor);
            async move {
                let res = send(server, "PUT", &format!("{path}/draft"), editor, Some(&json!({"revision": rev, "fields": {"body": body}}).to_string())).await;
                assert_eq!(res.status, 200, "{}", res.body);
            }
        };

        // 別の記事: 古い版で in_other_old を使い、その後の版では使わない。下書きで in_other_draft を使う
        let res = send(&server, "POST", &entries, &editor, Some(&create("other", doc("x", &[&in_other_old])))).await;
        let other = format!("{entries}/{}", parse(&res)["id"].as_str().unwrap());
        publish(other.clone(), 1).await;
        save(other.clone(), 1, doc("y", &[])).await;
        publish(other.clone(), 2).await;
        save(other.clone(), 2, doc("z", &[&in_other_draft])).await;

        // 対象の記事: v1 ですべて使い、v2（公開中）では in_current だけを使う
        let secret = "ERASE-ME-0123";
        let res = send(&server, "POST", &entries, &editor, Some(&create("target", doc(secret, &[&only_here, &in_other_draft, &in_current, &in_other_old])))).await;
        let id = parse(&res)["id"].as_str().unwrap().to_owned();
        let target = format!("{entries}/{id}");
        let v1 = publish(target.clone(), 1).await;
        save(target.clone(), 1, doc("v2", &[&in_current])).await;
        let v2 = publish(target.clone(), 2).await;
        // 翻訳（他のロケールの記事）
        let res = send(&server, "POST", &format!("{target}/translations"), &editor, Some("{\"locale\":\"en\"}")).await;
        let en = parse(&res)["id"].as_str().unwrap().to_owned();
        let en_before = get(&server, &format!("{entries}/{en}"), &admin).await.body;

        // 候補: 管理者だけが見られる。この版だけで使われているメディアと、他のロケールの記事
        let cand_path = format!("{target}/versions/{v1}/erase-candidates");
        assert_eq!(get(&server, &cand_path, &editor).await.status, 403);
        let cand = parse(&get(&server, &cand_path, &admin).await);
        assert_eq!(cand["media"], json!([only_here]));
        assert_eq!(cand["otherLocales"], json!([{"entryId": en, "locale": "en"}]));

        // 抹消: 編集者はできない。再認証が要る。公開中の版はできない（INV-PUB-05）
        let erase = |vid: &str| format!("{target}/versions/{vid}/erase");
        let body = |ids: &[&str]| json!({"deleteMediaIds": ids}).to_string();
        assert_eq!(send(&server, "POST", &erase(&v1), &editor, Some(&body(&[]))).await.status, 403);
        expire_reauth(&server.pool, "admin@example.test").await;
        let res = send(&server, "POST", &erase(&v1), &admin, Some(&body(&[]))).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (403, Some("reauth_required")));
        let admin = login_with_totp(&server, "admin@example.test", &totp).await;
        let res = send(&server, "POST", &erase(&v2), &admin, Some(&body(&[]))).await;
        assert_eq!((res.status, res.json_str("error").as_deref()), (409, Some("version_is_published")));
        assert!(parse(&get(&server, &format!("{target}/versions/{v2}"), &admin).await)["fields"].is_object());

        // 回答キャッシュ: v1 を引用したものと、v2 を引用したもの（抹消で消えるのは v1 を引用したものだけ。Q134）
        let db = yomi_server::workspace_db::lookup(&server.pool, ws).await.unwrap().unwrap();
        let cite = |vid: &str| yomi_server::answer::Source {
            citation: 1,
            entry_id: id.parse().unwrap(),
            locale: "ja".into(),
            url: None,
            anchor: Some("sec".into()),
            breadcrumb: Vec::new(),
            version_id: vid.parse().unwrap(),
            last_reviewed_at: None,
            review_expired: false,
        };
        let mut tx = yomi_server::workspace_db::begin(&server.pool, &db).await.unwrap();
        for (key, vid) in [(b"cites-v1", &v1), (b"cites-v2", &v2)] {
            yomi_server::answer::cache::store(&mut tx, key, "答え [1]", false, &[cite(vid)], &[]).await.unwrap();
        }
        tx.commit().await.unwrap();
        let cached_keys = || async {
            let mut tx = yomi_server::workspace_db::begin(&server.pool, &db).await.unwrap();
            let keys: Vec<Vec<u8>> = sqlx::query_scalar("SELECT key FROM answer_cache ORDER BY key").fetch_all(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
            keys
        };
        assert_eq!(cached_keys().await.len(), 2);

        // 抹消する。指定したメディアのうち、他で使われているものは消さない（INV-PUB-06）
        let random = uuid::Uuid::new_v4().to_string();
        let res = send(&server, "POST", &erase(&v1), &admin, Some(&body(&[&only_here, &in_other_draft, &random]))).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert_eq!(parse(&res)["deletedMediaIds"], json!([only_here]));
        assert_eq!(fetch_status(&server, &only_here_url).await, 404);
        assert_eq!(fetch_status(&server, &in_other_draft_url).await, 200);
        // 抹消した版を引用した回答キャッシュは消える
        assert_eq!(cached_keys().await, vec![b"cites-v2".to_vec()]);

        // 版は残り、本文だけが消える（INV-PUB-03, 04）
        let versions = parse(&get(&server, &format!("{target}/versions"), &admin).await);
        assert_eq!(versions.as_array().unwrap().len(), 2);
        let erased = versions.as_array().unwrap().iter().find(|v| v["id"] == v1.as_str()).unwrap();
        assert_eq!((erased["erased"].as_bool(), erased["number"].as_i64()), (Some(true), Some(1)));
        assert_eq!(erased["anchors"], json!(["sec"]), "metadata survives erasure");
        let detail = get(&server, &format!("{target}/versions/{v1}"), &admin).await;
        assert_eq!(detail.status, 200);
        assert_eq!(parse(&detail)["fields"], Value::Null);
        for path in [format!("{target}/versions"), format!("{target}/versions/{v1}"), target.clone(), entries.clone(), cand_path.clone()] {
            assert!(!get(&server, &path, &admin).await.body.contains(secret), "{path}");
        }
        // 抹消した版のメディアは、他の版の候補にもならない
        assert_eq!(parse(&get(&server, &cand_path, &admin).await)["media"], json!([]));
        // もう一度抹消しても同じ
        assert_eq!(send(&server, "POST", &erase(&v1), &admin, Some(&body(&[]))).await.status, 200);
        // 他のロケールの記事は変わらない（INV-LOC-01）
        assert_eq!(get(&server, &format!("{entries}/{en}"), &admin).await.body, en_before);

        // 非公開にすれば、元の公開中の版も抹消できる
        assert_eq!(send(&server, "POST", &format!("{target}/unpublish"), &editor, None).await.status, 204);
        let res = send(&server, "POST", &erase(&v2), &admin, Some(&body(&[&in_current]))).await;
        assert_eq!(res.status, 200, "{}", res.body);
        // in_current はこの記事の下書きでまだ使われている
        assert_eq!(parse(&res)["deletedMediaIds"], json!([]));

        // 監査ログ: 誰が行っても残る
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM yomi.audit_log WHERE workspace_id = $1 AND action = 'version.erased'")
            .bind(ws)
            .fetch_one(&server.pool)
            .await
            .unwrap();
        assert_eq!(n, 3);
        // 他の記事・存在しない版
        assert_eq!(send(&server, "POST", &format!("{other}/versions/{v1}/erase"), &admin, Some(&body(&[]))).await.status, 404);
        let _ = root;
    })
    .await;
}
