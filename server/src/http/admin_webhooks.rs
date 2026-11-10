//! 管理API: Webhook（01-architecture.md「Webhook」、13-security.md「外部への送信」）。ワークスペースの管理者・サーバー管理者（多要素認証）が扱う。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | GET | /api/admin/workspaces/{ws}/webhooks | 送信先の一覧（秘密の値は含めない） |
//! | POST | /api/admin/workspaces/{ws}/webhooks | 登録（`{url}`。再認証が必要。署名用の秘密はこの応答でだけ渡す） |
//! | DELETE | /api/admin/workspaces/{ws}/webhooks/{id} | 削除 |
//! | POST | /api/admin/workspaces/{ws}/webhooks/{id}/test | テスト送信（ジョブを積み、送信の記録のIDを返す） |
//! | POST | /api/admin/workspaces/{ws}/webhooks/{id}/rotate-secret | 秘密の入れ替え（再認証が必要。古い秘密でも24時間は署名する） |
//! | GET | /api/admin/workspaces/{ws}/webhooks/{id}/deliveries | 送信の記録（新しい順に50件。相手の応答は状態コードだけ） |
//!
//! - 登録のときにも、送信先を名前解決して検査する（内部向けなら断る）。送信のたびにも検査し直す。
//! - 管理APIの応答には、送信先が返した本文を含めない（Q112、INV-WH-04）。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use super::error::ApiError;
use super::extract::{AuthSession, JsonBody};
use crate::access;
use crate::audit::{self, action};
use crate::delivery::limiter::Subject;
use crate::webhooks;

/// Webhookごとの、1分あたりのテスト送信の上限。
const TESTS_PER_MINUTE: u32 = 10;

const MAX_URL_CHARS: usize = 2048;
/// 1つのワークスペースに登録できる送信先の数。
const MAX_WEBHOOKS: i64 = 20;
const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

fn parse_id(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|_| ApiError::not_found())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    id: Uuid,
    url: String,
    created_at: String,
    /// 秘密を入れ替えた後、古い秘密でも署名している間はその期限。
    previous_secret_until: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Vec<Summary>>, ApiError> {
    let ws = parse_id(&ws)?;
    access::workspace_admin(&state.pool, &current, ws).await??;
    let rows: Vec<(Uuid, String, String, Option<String>)> = sqlx::query_as(crate::sql!(
        "SELECT id, url, to_char(created_at AT TIME ZONE 'UTC', {RFC3339}) AS created_at,
                CASE WHEN previous_secret_until > now()
                     THEN to_char(previous_secret_until AT TIME ZONE 'UTC', {RFC3339}) END AS previous_secret_until
         FROM yomi.webhooks WHERE workspace_id = $1 ORDER BY created_at"
    ))
    .bind(ws)
    .fetch_all(&state.pool)
    .await
    .map_err(anyhow::Error::from)?;
    let rows = rows
        .into_iter()
        .map(|(id, url, created_at, previous_secret_until)| Summary {
            id,
            url,
            created_at,
            previous_secret_until,
        })
        .collect();
    Ok(Json(rows))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateWebhook {
    url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    id: Uuid,
    url: String,
    /// 署名用の秘密。この応答でだけ渡す。
    secret: String,
}

fn destination_refused(e: crate::egress::EgressError) -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "destination_not_allowed",
        format!(
            "This destination cannot receive webhooks ({}). Use an https:// URL that resolves to a public address, \
             or ask the server administrator to allow this internal destination.",
            e.code()
        ),
    )
}

pub async fn create(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<CreateWebhook>,
) -> Result<(StatusCode, Json<Created>), ApiError> {
    let ws = parse_id(&ws)?;
    access::workspace_admin(&state.pool, &current, ws).await??;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let url = req.url.trim().to_owned();
    if url.is_empty() || url.chars().count() > MAX_URL_CHARS || url.chars().any(char::is_control) {
        return Err(ApiError::bad_request(format!(
            "`url` must be 1 to {MAX_URL_CHARS} characters without control characters."
        )));
    }
    let egress = state
        .egress
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("outbound HTTP is not available"))?;
    // 登録の時点でも検査する（送るたびにも検査し直す）
    egress
        .for_workspace_destinations()
        .check(&url)
        .await
        .map_err(destination_refused)?;

    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    // 数の上限は、同じワークスペースの登録を順に処理して数える（ワークスペースごとのアドバイザリーロック）
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('yomi:webhooks:' || $1::text, 0))")
        .bind(ws)
        .execute(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM yomi.webhooks WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(&mut *tx)
            .await
            .map_err(anyhow::Error::from)?;
    if count >= MAX_WEBHOOKS {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "too_many_webhooks",
            format!("A workspace can have at most {MAX_WEBHOOKS} webhooks."),
        ));
    }
    let id = Uuid::new_v4();
    let secret = webhooks::generate_secret();
    let sealed = state
        .keys
        .encrypt(secret.as_bytes(), &webhooks::secret_aad(id))?;
    sqlx::query(
        "INSERT INTO yomi.webhooks (id, workspace_id, url, secret_enc, created_by) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(ws)
    .bind(&url)
    .bind(sealed)
    .bind(current.user_id)
    .execute(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;
    audit::record(
        &mut *tx,
        Some(ws),
        Some(current.user_id),
        action::WEBHOOK_CREATED,
        &[id],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok((StatusCode::CREATED, Json(Created { id, url, secret })))
}

pub async fn delete(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (ws, id) = (parse_id(&ws)?, parse_id(&id)?);
    access::workspace_admin(&state.pool, &current, ws).await??;
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    let deleted = sqlx::query("DELETE FROM yomi.webhooks WHERE id = $1 AND workspace_id = $2")
        .bind(id)
        .bind(ws)
        .execute(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?
        .rows_affected();
    if deleted == 0 {
        return Err(ApiError::not_found());
    }
    audit::record(
        &mut *tx,
        Some(ws),
        Some(current.user_id),
        action::WEBHOOK_DELETED,
        &[id],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn exists(state: &AppState, ws: Uuid, id: Uuid) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM yomi.webhooks WHERE id = $1 AND workspace_id = $2)",
    )
    .bind(id)
    .bind(ws)
    .fetch_one(&state.pool)
    .await
    .map_err(anyhow::Error::from)?)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestQueued {
    delivery_id: Uuid,
}

pub async fn test(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<TestQueued>), ApiError> {
    let (ws, id) = (parse_id(&ws)?, parse_id(&id)?);
    access::workspace_admin(&state.pool, &current, ws).await??;
    if !exists(&state, ws, id).await? {
        return Err(ApiError::not_found());
    }
    // 送信は取り出し手を使うので、回数を制限する（Webhookごとに1分あたり）
    state
        .delivery_limiter
        .check(
            Subject::WebhookTest {
                workspace: ws,
                webhook: id,
            },
            TESTS_PER_MINUTE,
        )
        .map_err(|retry| {
            ApiError::too_many(
                "webhook_test_rate_limited",
                "Too many test sends for this webhook. Try again later.",
                retry,
            )
        })?;
    // 送信はジョブで行う（同期の処理で外部と通信しない。原則5）。結果は送信の記録で状態コードだけを見る
    let delivery_id = webhooks::enqueue_test(&state.pool, &state.keys, ws, id).await?;
    Ok((StatusCode::ACCEPTED, Json(TestQueued { delivery_id })))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rotated {
    secret: String,
    previous_secret_until: String,
}

pub async fn rotate_secret(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<Rotated>, ApiError> {
    let (ws, id) = (parse_id(&ws)?, parse_id(&id)?);
    access::workspace_admin(&state.pool, &current, ws).await??;
    if !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    let old: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT secret_enc FROM yomi.webhooks WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
    )
    .bind(id)
    .bind(ws)
    .fetch_optional(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;
    let Some(old) = old else {
        return Err(ApiError::not_found());
    };
    let secret = webhooks::generate_secret();
    let sealed = state
        .keys
        .encrypt(secret.as_bytes(), &webhooks::secret_aad(id))?;
    let until: String = sqlx::query_scalar(crate::sql!(
        "UPDATE yomi.webhooks
         SET previous_secret_enc = $2, previous_secret_until = now() + make_interval(secs => $3),
             secret_enc = $4
         WHERE id = $1
         RETURNING to_char(previous_secret_until AT TIME ZONE 'UTC', {RFC3339})"
    ))
    .bind(id)
    .bind(old)
    .bind(webhooks::PREVIOUS_SECRET_SECONDS as f64)
    .bind(sealed)
    .fetch_one(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;
    audit::record(
        &mut *tx,
        Some(ws),
        Some(current.user_id),
        action::WEBHOOK_SECRET_ROTATED,
        &[id],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(Json(Rotated {
        secret,
        previous_secret_until: until,
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    id: Uuid,
    event_id: Uuid,
    event_type: String,
    entry_id: Option<Uuid>,
    /// pending, delivered, failed
    state: String,
    attempts: i32,
    /// 相手が返した状態コード（本文は記録しない）。
    last_status: Option<i32>,
    /// 失敗の種類。
    last_error: Option<String>,
    created_at: String,
    last_attempt_at: Option<String>,
}

type DeliveryRow = (
    Uuid,
    Uuid,
    String,
    Option<Uuid>,
    String,
    i32,
    Option<i32>,
    Option<String>,
    String,
    Option<String>,
);

pub async fn deliveries(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<Json<Vec<Delivery>>, ApiError> {
    let (ws, id) = (parse_id(&ws)?, parse_id(&id)?);
    access::workspace_admin(&state.pool, &current, ws).await??;
    if !exists(&state, ws, id).await? {
        return Err(ApiError::not_found());
    }
    let rows: Vec<DeliveryRow> = sqlx::query_as(crate::sql!(
        "SELECT id, event_id, event_type, entry_id, state, attempts, last_status, last_error,
                to_char(created_at AT TIME ZONE 'UTC', {RFC3339}) AS created_at,
                to_char(last_attempt_at AT TIME ZONE 'UTC', {RFC3339}) AS last_attempt_at
         FROM yomi.webhook_deliveries
         WHERE webhook_id = $1 AND workspace_id = $2
         ORDER BY created_at DESC LIMIT 50"
    ))
    .bind(id)
    .bind(ws)
    .fetch_all(&state.pool)
    .await
    .map_err(anyhow::Error::from)?;
    let rows = rows
        .into_iter()
        .map(
            |(
                id,
                event_id,
                event_type,
                entry_id,
                state,
                attempts,
                last_status,
                last_error,
                created_at,
                last_attempt_at,
            )| {
                Delivery {
                    id,
                    event_id,
                    event_type,
                    entry_id,
                    state,
                    attempts,
                    last_status,
                    last_error,
                    created_at,
                    last_attempt_at,
                }
            },
        )
        .collect();
    Ok(Json(rows))
}
