//! パスキー（WebAuthn）による多要素認証（13-security.md「ログイン」）。
//!
//! - 照合は webauthn-rs で行う（暗号処理を自作しない）。利用者の確認（生体認証やPIN）を必須にする。
//! - パスキーは多要素認証の方法の1つ。パスワードの代わりにはしない（ログインは、パスワードの後の2段目で使う）。
//! - 途中の状態（チャレンジ）はサーバーのDBにだけ置き、セッションと用途ごとに1つ、一度だけ使う。期限は5分。
//! - 署名の回数が戻った資格情報（複製の疑い）は、webauthn-rs が拒否する。照合のたびに回数を更新する。
//! - 同じ資格情報を別の利用者に登録させない（DBの一意制約）。

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;
use webauthn_rs::prelude::{
    CreationChallengeResponse, Passkey, PasskeyAuthentication, PasskeyRegistration,
    PublicKeyCredential, RegisterPublicKeyCredential, RequestChallengeResponse, Url, Webauthn,
    WebauthnBuilder,
};

use super::mfa;

/// 1人が登録できるパスキーの数の上限。
pub const MAX_PER_USER: i64 = 10;
/// 途中の状態の有効期限（秒）。
const CHALLENGE_SECONDS: f64 = 300.0;

const RFC3339: &str = r#"'YYYY-MM-DD"T"HH24:MI:SS"Z"'"#;

/// 公開URLから、照合の設定（RP ID はホスト名、オリジンは公開URLのオリジン）を作る。
pub fn relying_party(public_url: &str, name: &str) -> Result<Webauthn> {
    let url = Url::parse(public_url).context("public_url is not a valid URL")?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("public_url has no host"))?
        .to_owned();
    let origin = Url::parse(&url.origin().ascii_serialization())?;
    WebauthnBuilder::new(&host, &origin)
        .and_then(|b| b.rp_name(name).build())
        .map_err(|e| anyhow!("cannot configure passkeys for {public_url}: {e}"))
}

/// 途中の状態の用途。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Register,
    Login,
    Reauth,
}

impl Purpose {
    fn as_str(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::Login => "login",
            Self::Reauth => "reauth",
        }
    }
}

async fn save_challenge(
    pool: &PgPool,
    session_id: Uuid,
    purpose: Purpose,
    user_id: Uuid,
    state: &impl Serialize,
    name: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO cms.webauthn_challenges (session_id, purpose, user_id, state, name, expires_at)
         VALUES ($1, $2, $3, $4::jsonb, $5, now() + make_interval(secs => $6))
         ON CONFLICT (session_id, purpose) DO UPDATE
         SET user_id = EXCLUDED.user_id, state = EXCLUDED.state, name = EXCLUDED.name, expires_at = EXCLUDED.expires_at",
    )
    .bind(session_id)
    .bind(purpose.as_str())
    .bind(user_id)
    .bind(serde_json::to_string(state)?)
    .bind(name)
    .bind(CHALLENGE_SECONDS)
    .execute(pool)
    .await?;
    Ok(())
}

/// 途中の状態を取り出して消す（一度だけ使える）。期限切れ・他人のものなら None。
async fn take_challenge<T: DeserializeOwned>(
    pool: &PgPool,
    session_id: Uuid,
    purpose: Purpose,
    user_id: Uuid,
) -> Result<Option<(T, Option<String>)>> {
    let row: Option<(String, Option<String>, bool)> = sqlx::query_as(
        "DELETE FROM cms.webauthn_challenges WHERE session_id = $1 AND purpose = $2
         RETURNING state::text, name, user_id = $3 AND expires_at > now()",
    )
    .bind(session_id)
    .bind(purpose.as_str())
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some((state, name, true)) => Ok(Some((serde_json::from_str(&state)?, name))),
        _ => Ok(None),
    }
}

async fn credentials(pool: &PgPool, user_id: Uuid) -> Result<Vec<Passkey>> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT passkey::text FROM cms.passkeys WHERE user_id = $1 ORDER BY created_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|r| serde_json::from_str(r).map_err(Into::into))
        .collect()
}

/// 登録・照合を受け付けられなかった理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// 登録の数が上限に達している。
    TooMany,
    /// 始めていない・期限切れ・一度使った。
    NoChallenge,
    /// 応答の検証に失敗した。
    Invalid,
    /// その資格情報は登録済み。
    AlreadyRegistered,
    /// パスキーを登録していない。
    NoPasskeys,
}

/// 登録を始める。返す値をブラウザの `navigator.credentials.create()` に渡す。
pub async fn begin_registration(
    pool: &PgPool,
    rp: &Webauthn,
    session_id: Uuid,
    user_id: Uuid,
    email: &str,
    name: &str,
) -> Result<Result<CreationChallengeResponse, Refused>> {
    let existing = credentials(pool, user_id).await?;
    if existing.len() as i64 >= MAX_PER_USER {
        return Ok(Err(Refused::TooMany));
    }
    let exclude = existing.iter().map(|p| p.cred_id().clone()).collect();
    let (ccr, state) = rp
        .start_passkey_registration(user_id, email, email, Some(exclude))
        .map_err(|e| anyhow!("cannot start passkey registration: {e}"))?;
    save_challenge(
        pool,
        session_id,
        Purpose::Register,
        user_id,
        &state,
        Some(name),
    )
    .await?;
    Ok(Ok(ccr))
}

/// 登録の結果。
pub struct Registered {
    pub id: Uuid,
    /// 多要素認証の最初の方法だったときに発行した復旧コード（2つ目以降の方法なら空）。
    pub recovery_codes: Vec<String>,
}

/// 登録を終える。
pub async fn finish_registration(
    pool: &PgPool,
    rp: &Webauthn,
    session_id: Uuid,
    user_id: Uuid,
    response: &RegisterPublicKeyCredential,
) -> Result<Result<Registered, Refused>> {
    let Some((state, name)) =
        take_challenge::<PasskeyRegistration>(pool, session_id, Purpose::Register, user_id).await?
    else {
        return Ok(Err(Refused::NoChallenge));
    };
    let Ok(passkey) = rp.finish_passkey_registration(response, &state) else {
        return Ok(Err(Refused::Invalid));
    };
    let mut tx = pool.begin().await?;
    // 同じ人の同時の登録・解除と順番をそろえる
    let had_mfa: bool = sqlx::query_scalar(crate::sql!(
        "SELECT {} FROM cms.users u WHERE u.id = $1 FOR UPDATE",
        mfa::MFA_ENABLED_U
    ))
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM cms.passkeys WHERE user_id = $1")
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
    if count >= MAX_PER_USER {
        return Ok(Err(Refused::TooMany));
    }
    let id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO cms.passkeys (id, user_id, name, credential_id, passkey)
         VALUES ($1, $2, $3, $4, $5::jsonb)
         ON CONFLICT (credential_id) DO NOTHING",
    )
    .bind(id)
    .bind(user_id)
    .bind(name.unwrap_or_default())
    .bind(passkey.cred_id().as_ref())
    .bind(serde_json::to_string(&passkey)?)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted != 1 {
        return Ok(Err(Refused::AlreadyRegistered));
    }
    let recovery_codes = if had_mfa {
        Vec::new()
    } else {
        mfa::replace_recovery_codes(&mut tx, user_id).await?
    };
    tx.commit().await?;
    Ok(Ok(Registered { id, recovery_codes }))
}

/// 照合（ログインの2段目・再認証）を始める。返す値をブラウザの `navigator.credentials.get()` に渡す。
pub async fn begin_authentication(
    pool: &PgPool,
    rp: &Webauthn,
    session_id: Uuid,
    user_id: Uuid,
    purpose: Purpose,
) -> Result<Result<RequestChallengeResponse, Refused>> {
    let creds = credentials(pool, user_id).await?;
    if creds.is_empty() {
        return Ok(Err(Refused::NoPasskeys));
    }
    let (rcr, state) = rp
        .start_passkey_authentication(&creds)
        .map_err(|e| anyhow!("cannot start passkey authentication: {e}"))?;
    save_challenge(pool, session_id, purpose, user_id, &state, None).await?;
    Ok(Ok(rcr))
}

/// 照合を終える。合えば true（署名の回数と使った時刻を更新する）。
pub async fn finish_authentication(
    pool: &PgPool,
    rp: &Webauthn,
    session_id: Uuid,
    user_id: Uuid,
    purpose: Purpose,
    response: &PublicKeyCredential,
) -> Result<bool> {
    let Some((state, _)) =
        take_challenge::<PasskeyAuthentication>(pool, session_id, purpose, user_id).await?
    else {
        return Ok(false);
    };
    let Ok(result) = rp.finish_passkey_authentication(response, &state) else {
        return Ok(false);
    };
    let mut tx = pool.begin().await?;
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, passkey::text FROM cms.passkeys
         WHERE user_id = $1 AND credential_id = $2 FOR UPDATE",
    )
    .bind(user_id)
    .bind(result.cred_id().as_ref())
    .fetch_optional(&mut *tx)
    .await?;
    // 照合の後に解除されたなど
    let Some((id, stored)) = row else {
        return Ok(false);
    };
    let mut passkey: Passkey = serde_json::from_str(&stored)?;
    passkey.update_credential(&result);
    sqlx::query("UPDATE cms.passkeys SET passkey = $2::jsonb, last_used_at = now() WHERE id = $1")
        .bind(id)
        .bind(serde_json::to_string(&passkey)?)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// 一覧に出す情報。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub id: Uuid,
    pub name: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// 本人のパスキーの一覧。
pub async fn list(pool: &PgPool, user_id: Uuid) -> Result<Vec<Summary>> {
    let rows: Vec<(Uuid, String, String, Option<String>)> = sqlx::query_as(crate::sql!(
        "SELECT id, name, to_char(created_at AT TIME ZONE 'UTC', {RFC3339}),
                to_char(last_used_at AT TIME ZONE 'UTC', {RFC3339})
         FROM cms.passkeys WHERE user_id = $1 ORDER BY created_at"
    ))
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, created_at, last_used_at)| Summary {
            id,
            name,
            created_at,
            last_used_at,
        })
        .collect())
}

/// 本人のパスキーを1つ解除する。解除したら true。
/// 呼び出し側で、続けて [`mfa::forget_recovery_codes_if_no_method`] を呼ぶこと。
pub async fn remove(tx: &mut Transaction<'_, Postgres>, user_id: Uuid, id: Uuid) -> Result<bool> {
    sqlx::query("SELECT 1 FROM cms.users WHERE id = $1 FOR UPDATE")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    let removed = sqlx::query("DELETE FROM cms.passkeys WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(&mut **tx)
        .await?
        .rows_affected()
        == 1;
    Ok(removed)
}

/// 期限を過ぎた途中の状態を消す。
pub async fn sweep_challenges(pool: &PgPool) -> Result<()> {
    sqlx::query("DELETE FROM cms.webauthn_challenges WHERE expires_at < now()")
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relying_party_from_public_url() {
        let rp = relying_party("https://cms.example.test", "cms.example.test").unwrap();
        let (_ccr, state) = rp
            .start_passkey_registration(
                Uuid::new_v4(),
                "a@example.test",
                "a@example.test",
                Some(vec![]),
            )
            .unwrap();
        serde_json::to_string(&state).unwrap();
        assert!(relying_party("not a url", "x").is_err());
    }
}
