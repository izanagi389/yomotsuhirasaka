//! 外部のアカウントのひも付けの一覧の結合テスト（本物のPostgreSQLとHTTPを使う）。
//!
//! 環境変数 `YOMI_TEST_PG_SUPERUSER_URL` があるときだけ実行する。

mod common;

use common::server::*;
use common::with_scratch;

#[tokio::test]
async fn listing_my_identities_shows_issuers_but_not_subjects() {
    with_scratch(|app, migration| async move {
        let server = start(config(&app, &migration)).await;
        create_user(&server.pool, "a@example.test", false).await;
        create_user(&server.pool, "b@example.test", false).await;
        let a = login(&server, "a@example.test", PASSWORD).await;
        let res = get(&server, "/api/admin/me/identities", &a).await;
        assert_eq!((res.status, res.body.as_str()), (200, "[]"));
        // ひも付けを作る処理はまだないので、DBに直接入れて確かめる
        for (email, sub) in [("a@example.test", "sub-of-a-123"), ("b@example.test", "sub-of-b-456")] {
            sqlx::query(
                "INSERT INTO yomi.external_identities (id, user_id, issuer, subject)
                 SELECT gen_random_uuid(), id, 'https://idp.example.test', $2 FROM yomi.users WHERE email = $1",
            )
            .bind(email)
            .bind(sub)
            .execute(&server.pool)
            .await
            .unwrap();
        }
        let res = get(&server, "/api/admin/me/identities", &a).await;
        assert_eq!(res.status, 200);
        assert!(res.body.contains("https://idp.example.test"), "{}", res.body);
        assert!(!res.body.contains("sub-of-a") && !res.body.contains("sub-of-b"), "{}", res.body);
        assert_eq!(res.body.matches("\"issuer\"").count(), 1, "{}", res.body);
    })
    .await;
}
