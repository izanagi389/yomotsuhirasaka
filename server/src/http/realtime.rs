//! 管理画面への即時の知らせ（WebSocket。03-content-and-editing.md「同時編集（Q73）」、計画の10番）。
//!
//! - 接続先は `/api/admin/ws` の1つ。セッションのCookieで認証し、個人トークンでは接続できない。接続を始めるときに `Origin` が
//!   CMSの公開URLと同じかを確かめ、接続の後の最初のメッセージで CSRF トークンを確かめる（5秒以内。それまで何も送らない）。
//! - 1つのセッションの同時接続は 10 本まで、画面から届く1通は 16KB まで。セッションが失効したら接続を切る。
//! - 知らせはIDと種類だけ（内容は載せない）。画面は知らせを受けたら、今までのAPIで読み直す（権限の照合もそこで行う）。
//!   送る前に、その人がいまもワークスペースに所属しているかを照合する。
//! - プロセスをまたぐ知らせは PostgreSQL の LISTEN/NOTIFY で配る（このプロセスの知らせも同じ道を通る）。
//! - ロックの操作などは HTTP のAPIで行い、ここは知らせを送るだけ（画面からのメッセージは CSRF トークンのほかは読まない）。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use tokio::sync::broadcast;
use uuid::Uuid;

use super::AppState;
use super::error::ApiError;
use super::extract::AuthSession;
use crate::auth::session::{self, Current};

/// LISTEN/NOTIFY のチャンネル。
const CHANNEL: &str = "cms_events";
/// 1つのセッションの同時接続の上限。
const MAX_CONNECTIONS_PER_SESSION: usize = 10;
/// 画面から届く1通の大きさの上限。
const MAX_MESSAGE_BYTES: usize = 16 * 1024;
/// 接続の後、CSRF トークンを待つ長さ。
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
/// セッションが有効かを確かめ直す間隔。
const SESSION_CHECK: Duration = Duration::from_secs(30);

/// 画面への知らせ（IDと種類だけ）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    /// 種類（`editLock`: 記事の編集のロックが変わった）。
    #[serde(rename = "type")]
    pub kind: String,
    pub workspace_id: Uuid,
    pub entry_id: Uuid,
}

/// このプロセスの知らせの配り先と、セッションごとの接続の数。
pub struct Events {
    tx: broadcast::Sender<Event>,
    connections: Mutex<HashMap<Uuid, usize>>,
}

impl Default for Events {
    fn default() -> Self {
        Self {
            tx: broadcast::channel(1024).0,
            connections: Mutex::new(HashMap::new()),
        }
    }
}

/// 知らせを出す（NOTIFY。すべてのプロセスの LISTEN に届く）。失敗しても操作は止めない。
pub async fn publish(pool: &PgPool, event: Event) {
    let payload = match serde_json::to_string(&event) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("cannot encode a realtime event: {e:#}");
            return;
        }
    };
    if let Err(e) = sqlx::query("SELECT pg_notify($1, $2)")
        .bind(CHANNEL)
        .bind(payload)
        .execute(pool)
        .await
    {
        tracing::error!("cannot publish a realtime event: {e:#}");
    }
}

/// LISTEN して、届いた知らせをこのプロセスの接続に配る（切れたらつなぎ直す）。
pub fn spawn_listener(state: AppState) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = listen(&state).await {
                tracing::warn!("realtime listener stopped: {e:#}");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

async fn listen(state: &AppState) -> anyhow::Result<()> {
    let mut listener = PgListener::connect_with(&state.pool).await?;
    listener.listen(CHANNEL).await?;
    loop {
        let n = listener.recv().await?;
        if let Ok(event) = serde_json::from_str::<Event>(n.payload()) {
            // 受け取る接続がなければ送れないだけ
            let _ = state.events.tx.send(event);
        }
    }
}

/// 接続を数える（上限を超えたら None）。外すのは [`ConnectionGuard`] が行う。
struct ConnectionGuard {
    state: AppState,
    session_id: Uuid,
}

impl ConnectionGuard {
    fn new(state: &AppState, session_id: Uuid) -> Option<Self> {
        let mut map = state.events.connections.lock().expect("connections lock");
        let n = map.entry(session_id).or_insert(0);
        if *n >= MAX_CONNECTIONS_PER_SESSION {
            return None;
        }
        *n += 1;
        Some(Self {
            state: state.clone(),
            session_id,
        })
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let mut map = self
            .state
            .events
            .connections
            .lock()
            .expect("connections lock");
        if let Some(n) = map.get_mut(&self.session_id) {
            *n -= 1;
            if *n == 0 {
                map.remove(&self.session_id);
            }
        }
    }
}

/// `GET /api/admin/ws`
pub async fn connect(
    State(state): State<AppState>,
    AuthSession(current): AuthSession,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    if current.via_token() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "session_required",
            "Realtime updates are only available from the admin screen.",
        ));
    }
    // ほかのサイトからの接続を断る（ブラウザは WebSocket の接続に必ず Origin を付ける）
    let origin_ok = headers
        .get(header::ORIGIN)
        .is_some_and(|v| v.as_bytes() == state.public_origin.as_bytes());
    if !origin_ok {
        return Err(ApiError::forbidden_origin());
    }
    let Some(guard) = ConnectionGuard::new(&state, current.id) else {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_connections",
            "Too many realtime connections for this session.",
        ));
    };
    Ok(ws
        .max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| async move {
            run(state, current, socket).await;
            drop(guard);
        }))
}

/// 画面から届く最初のメッセージ。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Hello {
    #[serde(rename = "type")]
    kind: String,
    csrf_token: String,
}

async fn close(mut socket: WebSocket, code: u16, reason: &'static str) {
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
}

async fn run(state: AppState, current: Current, mut socket: WebSocket) {
    // 1. CSRF トークン（それまで何も送らない）
    let authed = match tokio::time::timeout(AUTH_TIMEOUT, socket.recv()).await {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<Hello>(&text)
            .is_ok_and(|h| h.kind == "hello" && current.csrf_matches(&h.csrf_token)),
        _ => false,
    };
    if !authed {
        close(socket, 4401, "unauthenticated").await;
        return;
    }
    if socket
        .send(Message::Text(r#"{"type":"ready"}"#.into()))
        .await
        .is_err()
    {
        return;
    }
    // 2. 知らせを配る
    let mut rx = state.events.tx.subscribe();
    let mut check = tokio::time::interval(SESSION_CHECK);
    check.tick().await;
    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                // 画面からのメッセージは読まない（ping/pong は下の層が返す）
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {}
            },
            event = rx.recv() => match event {
                Ok(event) => {
                    if !may_receive(&state, &current, event.workspace_id).await {
                        continue;
                    }
                    let Ok(text) = serde_json::to_string(&event) else { continue };
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                // 取りこぼしは、画面がつなぎ直したときに読み直す
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = check.tick() => {
                match session::is_active(&state.pool, current.id, current.user_id).await {
                    Ok(true) => {}
                    _ => {
                        close(socket, 4401, "session ended").await;
                        return;
                    }
                }
            }
        }
    }
}

/// 知らせを受け取ってよいか（いまも所属しているか、所属していないサーバー管理者として見られるか）。送るたびに照合する。
async fn may_receive(state: &AppState, current: &Current, workspace_id: Uuid) -> bool {
    matches!(
        crate::access::member(&state.pool, current, workspace_id).await,
        Ok(Ok(_))
    )
}
