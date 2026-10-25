//! 管理API: ワークスペースの監査ログの書き出し先（Webhook。11-permissions-and-audit.md「外部への書き出し」、計画の14番）。
//!
//! | メソッド | パス | 内容 |
//! |---|---|---|
//! | GET | /api/admin/workspaces/{ws}/audit-sinks | 書き出し先の一覧（状態: 送れていない理由、停止） |
//! | POST | /api/admin/workspaces/{ws}/audit-sinks | 作る（`{url}`）。署名の秘密は、この応答で一度だけ返す |
//! | DELETE | /api/admin/workspaces/{ws}/audit-sinks/{id} | 消す |
//! | POST | /api/admin/workspaces/{ws}/audit-sinks/{id}/resume | 停止したものを再開する（続きから送る） |
//!
//! - そのワークスペースの管理者とサーバー管理者だけ。変更は再認証が要り、監査ログに残る。
//! - 届くのは、そのワークスペースの記録だけ（作った時点より後のもの。INV-AUD-03）。送るのは audit_export.rs。
//! - 宛先は記事の Webhook と同じく、登録の時点と送るたびに外部への送信の仕組み（egress）で確かめる。

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
use crate::audit_export;

/// ワークスペースあたりの書き出し先の上限。
const MAX_SINKS: i64 = 5;
/// URL の長さの上限。
const MAX_URL_CHARS: usize = 2048;

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

fn parse_id(id: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(id).map_err(|_| ApiError::not_found())
}

async fn admin(
    state: &AppState,
    current: &crate::auth::session::Current,
    ws: Uuid,
    change: bool,
) -> Result<(), ApiError> {
    access::workspace_admin(&state.pool, current, ws).await??;
    if change && !current.reauth_fresh {
        return Err(ApiError::reauth_required());
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sink {
    id: Uuid,
    url: String,
    created_at: String,
    /// 送れなくなった時刻（送れていれば null）。
    failing_since: Option<String>,
    /// 最後の失敗の理由のコード（相手の応答の本文は記録しない）。
    last_error: Option<String>,
    /// 停止した時刻（7日間続けて送れなかったとき）。
    stopped_at: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
) -> Result<Json<Vec<Sink>>, ApiError> {
    let ws = parse_id(&ws)?;
    admin(&state, &current, ws, false).await?;
    type Row = (
        Uuid,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<Row> = sqlx::query_as(crate::sql!(
        "SELECT id, url, to_char(created_at AT TIME ZONE 'UTC', {RFC3339}),
                to_char(failing_since AT TIME ZONE 'UTC', {RFC3339}), last_error,
                to_char(stopped_at AT TIME ZONE 'UTC', {RFC3339})
         FROM cms.audit_sinks WHERE workspace_id = $1 ORDER BY created_at"
    ))
    .bind(ws)
    .fetch_all(&state.pool)
    .await
    .map_err(anyhow::Error::from)?;
    Ok(Json(
        rows.into_iter()
            .map(
                |(id, url, created_at, failing_since, last_error, stopped_at)| Sink {
                    id,
                    url,
                    created_at,
                    failing_since,
                    last_error,
                    stopped_at,
                },
            )
            .collect(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Create {
    url: String,
}

#[derive(Serialize)]
pub struct Created {
    id: Uuid,
    url: String,
    secret: String,
}

fn destination_refused(e: crate::egress::EgressError) -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "destination_not_allowed",
        format!(
            "This destination cannot receive audit records ({}). Use an https:// URL that resolves to a public address, \
             or ask the server administrator to allow this internal destination.",
            e.code()
        ),
    )
}

pub async fn create(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<Create>,
) -> Result<(StatusCode, Json<Created>), ApiError> {
    let ws = parse_id(&ws)?;
    admin(&state, &current, ws, true).await?;
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
    egress.check(&url).await.map_err(destination_refused)?;

    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('cms:audit-sinks:' || $1::text, 0))",
    )
    .bind(ws)
    .execute(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM cms.audit_sinks WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(&mut *tx)
            .await
            .map_err(anyhow::Error::from)?;
    if count >= MAX_SINKS {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "too_many_audit_sinks",
            format!("A workspace can have at most {MAX_SINKS} audit log destinations."),
        ));
    }
    let id = Uuid::new_v4();
    let (secret, sealed) = audit_export::new_secret(&state.keys, id)?;
    sqlx::query(
        "INSERT INTO cms.audit_sinks (id, workspace_id, url, secret_enc, created_by) VALUES ($1, $2, $3, $4, $5)",
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
        action::AUDIT_SINK_CREATED,
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
    admin(&state, &current, ws, true).await?;
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    let done = sqlx::query("DELETE FROM cms.audit_sinks WHERE id = $1 AND workspace_id = $2")
        .bind(id)
        .bind(ws)
        .execute(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?
        .rows_affected()
        > 0;
    if !done {
        return Err(ApiError::not_found());
    }
    audit::record(
        &mut *tx,
        Some(ws),
        Some(current.user_id),
        action::AUDIT_SINK_DELETED,
        &[id],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn resume(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    Path((ws, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (ws, id) = (parse_id(&ws)?, parse_id(&id)?);
    admin(&state, &current, ws, true).await?;
    let mut tx = state.pool.begin().await.map_err(anyhow::Error::from)?;
    let done = sqlx::query(
        "UPDATE cms.audit_sinks SET stopped_at = NULL, failing_since = NULL, last_error = NULL, last_attempt_at = NULL
         WHERE id = $1 AND workspace_id = $2",
    )
    .bind(id)
    .bind(ws)
    .execute(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?
    .rows_affected()
        > 0;
    if !done {
        return Err(ApiError::not_found());
    }
    audit::record(
        &mut *tx,
        Some(ws),
        Some(current.user_id),
        action::AUDIT_SINK_RESUMED,
        &[id],
    )
    .await?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(StatusCode::NO_CONTENT)
}
