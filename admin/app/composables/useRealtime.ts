// サーバーからの即時の知らせ（WebSocket。03-content-and-editing.md「同時編集（Q73）」、server/src/http/realtime.rs）。
// - アプリ全体で1本の接続を共有する。接続したら、最初のメッセージで CSRF トークンを送る（URL には載せない）
// - 知らせはIDと種類だけ。受け取った画面は、今までのAPIで読み直す
// - 切れたら、間隔をあけてつなぎ直す（つなぎ直したときは、取りこぼしに備えて "reconnected" を知らせる）

export interface RealtimeEvent {
  type: string;
  workspaceId: string;
  entryId: string;
}

type Listener = (event: RealtimeEvent | { type: "reconnected" }) => void;

let socket: WebSocket | null = null;
let listeners = new Set<Listener>();
let retryMs = 1000;
let retryTimer: ReturnType<typeof setTimeout> | undefined;
let everConnected = false;

/** 接続先（管理APIと同じオリジンの /api/admin/ws）。 */
function url(): string {
  const u = new URL("/api/admin/ws", window.location.href);
  u.protocol = u.protocol === "https:" ? "wss:" : "ws:";
  return u.toString();
}

function connect() {
  if (socket || listeners.size === 0) return;
  const token = csrfToken();
  if (!token) return;
  const ws = new WebSocket(url());
  socket = ws;
  ws.onopen = () => ws.send(JSON.stringify({ type: "hello", csrfToken: token }));
  ws.onmessage = (e) => {
    let msg: { type?: unknown };
    try {
      msg = JSON.parse(String(e.data));
    } catch {
      return;
    }
    if (msg.type === "ready") {
      retryMs = 1000;
      if (everConnected) for (const l of listeners) l({ type: "reconnected" });
      everConnected = true;
      return;
    }
    for (const l of listeners) l(msg as RealtimeEvent);
  };
  ws.onclose = () => {
    socket = null;
    if (listeners.size === 0) return;
    clearTimeout(retryTimer);
    retryTimer = setTimeout(connect, retryMs);
    retryMs = Math.min(retryMs * 2, 30_000);
  };
}

/** 知らせを受け取る（画面を離れたら自動で外す）。 */
export function useRealtime(listener: Listener) {
  onMounted(() => {
    listeners.add(listener);
    connect();
  });
  onBeforeUnmount(() => {
    listeners.delete(listener);
    if (listeners.size === 0) {
      clearTimeout(retryTimer);
      socket?.close();
      socket = null;
    }
  });
}

/** ログアウトのときなどに、接続を閉じる。 */
export function closeRealtime() {
  listeners = new Set();
  clearTimeout(retryTimer);
  socket?.close();
  socket = null;
  everConnected = false;
}
