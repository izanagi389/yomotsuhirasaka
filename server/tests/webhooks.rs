//! Webhookの結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。
//! 受け手は手元（127.0.0.1）で待ち受け、外部への送信の例外として許す。

mod common;

use std::sync::{Arc, Mutex};

use common::content::*;
use common::server::*;
use common::with_scratch;
use hmac::{KeyInit as _, Mac as _};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use yomi_server::worker;

/// 届いたリクエスト（生のまま）を記録し、200 と目印入りの本文を返す受け手。
async fn receiver() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            let log = log.clone();
            tokio::spawn(async move {
                let mut got = Vec::new();
                let mut buf = vec![0u8; 65536];
                loop {
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&got);
                    if let Some(at) = text.find("\r\n\r\n") {
                        let len = text[..at]
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if got.len() >= at + 4 + len {
                            break;
                        }
                    }
                }
                log.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&got).into_owned());
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 21\r\nconnection: close\r\n\r\nRECEIVER-SECRET-REPLY")
                    .await;
            });
        }
    });
    (port, seen)
}

fn parse(res: &Resp) -> Value {
    serde_json::from_str(&res.body).unwrap_or_else(|_| panic!("{}", res.body))
}

fn split(raw: &str) -> (String, String) {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    (head.to_owned(), body.to_owned())
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

#[tokio::test]
async fn webhooks_are_signed_and_carry_no_content() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.egress.workspace_internal_destination_exceptions = vec!["127.0.0.1".to_owned()];
        let server = start(cfg).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        create_user(&server.pool, "editor@example.test", false).await;
        add_member(&server, ws, "editor@example.test", "editor").await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let hooks = format!("/api/admin/workspaces/{ws}/webhooks");
        let (port, seen) = receiver().await;

        // 管理者（とサーバー管理者）だけが登録できる。内部向けの宛先は、例外でなければ登録の時点で断る
        let body = |url: &str| json!({"url": url}).to_string();
        let url = format!("http://127.0.0.1:{port}/hook");
        assert_eq!(send(&server, "POST", &hooks, &editor, Some(&body(&url))).await.status, 403);
        for bad in ["http://10.0.0.1/", "https://169.254.169.254/", "https://[::1]/", "ftp://example.com/", "https://user:pw@example.com/"] {
            let res = send(&server, "POST", &hooks, &root, Some(&body(bad))).await;
            assert_eq!((res.status, res.json_str("error").as_deref()), (422, Some("destination_not_allowed")), "{bad}: {}", res.body);
        }
        let res = send(&server, "POST", &hooks, &root, Some(&body(&url))).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let hook_id = res.json_str("id").unwrap();
        let secret = res.json_str("secret").unwrap();
        // 一覧には秘密を出さない
        let list = get(&server, &hooks, &root).await;
        assert_eq!(list.status, 200);
        assert!(!list.body.contains(&secret), "{}", list.body);

        // 記事を作って公開する
        let entries = format!("/api/admin/workspaces/{ws}/entries");
        let create = json!({"contentType": "post", "fields": {"title": "BODY-MARKER-TITLE", "slug": "first",
            "body": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "BODY-MARKER"}]}]}}})
        .to_string();
        let res = send(&server, "POST", &entries, &editor, Some(&create)).await;
        assert_eq!(res.status, 201, "{}", res.body);
        let id = res.json_str("id").unwrap();
        let publish = |rev: i64| json!({"revision": rev, "correction": false}).to_string();
        let res = send(&server, "POST", &format!("{entries}/{id}/publish"), &editor, Some(&publish(1))).await;
        assert_eq!(res.status, 200, "{}", res.body);
        while worker::run_once(&server.state).await.unwrap() {}

        let raw = seen.lock().unwrap().first().cloned().expect("no webhook received");
        let (head, payload) = split(&raw);
        assert!(head.starts_with("POST /hook HTTP/1.1"), "{head}");
        let v: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["type"], "entry.published");
        assert_eq!(v["workspaceId"], ws.to_string());
        assert_eq!(v["entryId"], id);
        assert_eq!(v["locale"], "ja");
        assert_eq!(v["url"], "https://site.example.test/ja/blog/first");
        assert!(v["translationGroupId"].is_string());
        // 本文や題名は含めない（INV-WH-07）
        assert!(!raw.contains("BODY-MARKER"), "{raw}");
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 6, "{keys:?}");

        // 署名: t=<秒>,id=<イベントID>,v1=<HMAC-SHA256(秘密, "t.id.本文")>（INV-WH-03）
        let sig = header(&head, "webhook-signature").expect("signature header");
        let parts: Vec<(&str, &str)> = sig.split(',').map(|p| p.split_once('=').unwrap()).collect();
        let t = parts.iter().find(|p| p.0 == "t").unwrap().1;
        let event_id = parts.iter().find(|p| p.0 == "id").unwrap().1;
        let v1 = parts.iter().find(|p| p.0 == "v1").unwrap().1;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        assert!((t.parse::<i64>().unwrap() - now).abs() < 300);
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(format!("{t}.{event_id}.{payload}").as_bytes());
        let expected: String = mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(v1, expected);

        // 送信の記録には状態コードだけが残り、相手の応答の本文は出ない（INV-WH-04）
        let deliveries = get(&server, &format!("{hooks}/{hook_id}/deliveries"), &root).await;
        assert_eq!(deliveries.status, 200);
        let d = parse(&deliveries);
        assert_eq!(d[0]["state"], "delivered");
        assert_eq!(d[0]["lastStatus"], 200);
        assert_eq!(d[0]["eventId"], event_id);
        assert!(!deliveries.body.contains("RECEIVER-SECRET-REPLY"));

        // テスト送信もジョブで送る。応答には相手の応答を含めない
        let res = send(&server, "POST", &format!("{hooks}/{hook_id}/test"), &root, None).await;
        assert_eq!(res.status, 202, "{}", res.body);
        assert!(!res.body.contains("RECEIVER-SECRET-REPLY"));
        while worker::run_once(&server.state).await.unwrap() {}
        assert_eq!(seen.lock().unwrap().len(), 2);
        assert!(seen.lock().unwrap()[1].contains("\"type\":\"webhook.test\""));

        // 秘密を入れ替えると、しばらくは新旧の両方で署名する
        let res = send(&server, "POST", &format!("{hooks}/{hook_id}/rotate-secret"), &root, None).await;
        assert_eq!(res.status, 200, "{}", res.body);
        let res = send(&server, "POST", &format!("{entries}/{id}/unpublish"), &editor, None).await;
        assert_eq!(res.status, 204, "{}", res.body);
        while worker::run_once(&server.state).await.unwrap() {}
        let raw = seen.lock().unwrap()[2].clone();
        let (head, payload) = split(&raw);
        assert_eq!(serde_json::from_str::<Value>(&payload).unwrap()["type"], "entry.unpublished");
        let sig = header(&head, "webhook-signature").unwrap();
        assert_eq!(sig.matches("v1=").count(), 2, "{sig}");

        // 削除すると送らない
        assert_eq!(send(&server, "DELETE", &format!("{hooks}/{hook_id}"), &root, None).await.status, 204);
        let res = send(&server, "DELETE", &format!("{entries}/{id}"), &editor, None).await;
        assert_eq!(res.status, 204, "{}", res.body);
        while worker::run_once(&server.state).await.unwrap() {}
        assert_eq!(seen.lock().unwrap().len(), 3);
    })
    .await;
}

#[tokio::test]
async fn publish_settings_are_saved_and_announced() {
    with_scratch(|app, migration| async move {
        let mut cfg = config(&app, &migration);
        cfg.egress.workspace_internal_destination_exceptions = vec!["127.0.0.1".to_owned()];
        let server = start(cfg).await;
        let (root, token) = root_with_deploy_token(&server).await;
        assert_eq!(deploy(&server, &token, "blog", &blog_manifest(), "").await.status, 200);
        let ws = workspace_with_template(&server, &root, "A", "blog").await;
        for (email, role) in [
            ("admin@example.test", "admin"),
            ("editor@example.test", "editor"),
            ("author@example.test", "author"),
        ] {
            create_user(&server.pool, email, false).await;
            add_member(&server, ws, email, role).await;
        }
        let admin = login(&server, "admin@example.test", PASSWORD).await;
        enroll(&server, &admin).await;
        let editor = login(&server, "editor@example.test", PASSWORD).await;
        let author = login(&server, "author@example.test", PASSWORD).await;
        let (port, seen) = receiver().await;
        let hooks = format!("/api/admin/workspaces/{ws}/webhooks");
        let res = send(&server, "POST", &hooks, &root, Some(&json!({"url": format!("http://127.0.0.1:{port}/s")}).to_string())).await;
        assert_eq!(res.status, 201, "{}", res.body);

        let entries = format!("/api/admin/workspaces/{ws}/entries");
        let create = json!({"contentType": "post", "fields": {"title": "t", "slug": "s"}}).to_string();
        let id = send(&server, "POST", &entries, &editor, Some(&create)).await.json_str("id").unwrap();
        let path = format!("{entries}/{id}/publish-settings");

        // 既定値。読むのは所属者全員
        let res = get(&server, &path, &author).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert_eq!(parse(&res), json!({"useForChatbot": true, "includeInAiMaterials": true, "optOutOfAiTraining": false}));
        // 変えられるのは重い権限 policySwitches を持つ人（既定は管理者だけ。編集者・執筆者はできない）
        let change = json!({"includeInAiMaterials": false, "optOutOfAiTraining": true}).to_string();
        assert_eq!(send(&server, "PATCH", &path, &author, Some(&change)).await.status, 403);
        assert_eq!(send(&server, "PATCH", &path, &editor, Some(&change)).await.status, 403);
        assert_eq!(parse(&get(&server, &path, &editor).await)["optOutOfAiTraining"], false);
        assert_eq!(send(&server, "PATCH", &path, &admin, Some("{}")).await.status, 400);
        assert_eq!(send(&server, "PATCH", &path, &admin, Some("{\"unknown\":true}")).await.status, 400);
        let res = send(&server, "PATCH", &path, &admin, Some(&change)).await;
        assert_eq!(res.status, 200, "{}", res.body);
        assert_eq!(parse(&res), json!({"useForChatbot": true, "includeInAiMaterials": false, "optOutOfAiTraining": true}));
        assert_eq!(parse(&get(&server, &path, &editor).await)["optOutOfAiTraining"], true);
        // 同じ値をもう一度送っても、イベントは増えない
        assert_eq!(send(&server, "PATCH", &path, &admin, Some(&change)).await.status, 200);
        while worker::run_once(&server.state).await.unwrap() {}
        let got = seen.lock().unwrap().clone();
        assert_eq!(got.len(), 1, "{got:?}");
        let (_, payload) = split(&got[0]);
        let v: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["type"], "entry.settings_changed");
        assert_eq!(v["entryId"], id);
        // 公開していない記事の公開URLは送らない
        assert!(v.get("url").is_none(), "{v}");
    })
    .await;
}
