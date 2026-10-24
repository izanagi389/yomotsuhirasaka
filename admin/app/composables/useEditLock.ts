// 記事の編集のロック（ソフトロック。03-content-and-editing.md「同時編集（Q73）」、server/src/http/admin_locks.rs）。
// - 編集できる人が記事の画面を開いたら、ロックを取る。取れなければ（ほかの画面が編集中なら）閲覧のみ
// - 開いている間は生存を知らせる。タブが裏に回って10分たったら知らせるのをやめる（開きっぱなしで、ほかの人を止めないため）
// - ロックが変わった知らせ（WebSocket）を受けたら読み直す。引き継がれたら閲覧のみにし、保存できなかった入力は画面に残す
// - 画面を離れたら、すぐにロックを外す
// 保存を断るかどうかはサーバーが決める（423）。ここは画面の出し分けと、保存に付けるロックのIDを持つだけ。

export interface LockHolder {
  userId: string;
  /** 表示名（未設定なら null = 「他の所属者」）。 */
  name: string | null;
  sameUser: boolean;
}

interface LockState {
  holder: LockHolder | null;
  isMine: boolean;
  acquiredAt: string | null;
  expiresAt: string | null;
}

/** mine: 自分が編集中。held: ほかの画面が編集中。lost: 引き継がれた。free: 誰も編集していない（閲覧のみの人）。 */
export type LockStatus = "loading" | "mine" | "held" | "lost" | "free";

const HIDDEN_LIMIT_MS = 10 * 60 * 1000;
const VIEWER_REFRESH_MS = 30 * 1000;

export function useEditLock(workspaceId: string, entryId: string, canEdit: () => boolean) {
  const api = useApi();
  const base = `/workspaces/${workspaceId}/entries/${entryId}/edit-lock`;
  const lockId = ref<string | null>(null);
  const status = ref<LockStatus>("loading");
  const holder = ref<LockHolder | null>(null);
  const since = ref<string | null>(null);
  const error = ref<unknown>(null);
  let heartbeatTimer: ReturnType<typeof setInterval> | undefined;
  let viewerTimer: ReturnType<typeof setInterval> | undefined;
  let hiddenSince: number | null = null;

  async function readState(mine: string | null = lockId.value) {
    const s = await api.get<LockState>(`${base}${mine ? `?mine=${mine}` : ""}`);
    holder.value = s.holder;
    since.value = s.acquiredAt;
    return s;
  }

  function startHeartbeat(seconds: number) {
    clearInterval(heartbeatTimer);
    heartbeatTimer = setInterval(() => void heartbeat(), seconds * 1000);
  }

  async function acquire(): Promise<boolean> {
    try {
      const got = await api.post<{ lockId: string; heartbeatSeconds: number }>(base);
      lockId.value = got.lockId;
      status.value = "mine";
      startHeartbeat(got.heartbeatSeconds);
      await readState();
      return true;
    } catch (e) {
      if (e instanceof ApiError && e.code === "entry_locked") {
        status.value = "held";
        await readState();
        return false;
      }
      error.value = e;
      return false;
    }
  }

  async function heartbeat() {
    if (!lockId.value || status.value !== "mine") return;
    // 裏に回って10分たったら知らせない（ロックは期限で外れる）
    if (hiddenSince !== null && Date.now() - hiddenSince > HIDDEN_LIMIT_MS) return;
    try {
      await api.post(`${base}/${lockId.value}/heartbeat`);
    } catch (e) {
      if (e instanceof ApiError && e.code === "lock_lost") await refresh();
    }
  }

  /** 読み直す（知らせを受けたとき・つなぎ直したとき・閲覧のみの人の定期的な確認）。 */
  async function refresh() {
    try {
      // 問い合わせの間に自分で引き継いだときなど、ロックのIDが変わっていたら、この答えでは判定しない（古い答えのため）
      const asked = lockId.value;
      const s = await readState(asked);
      if (lockId.value !== asked) return;
      if (status.value === "mine" && !s.isMine) {
        if (s.holder) {
          // 引き継がれた
          status.value = "lost";
          lockId.value = null;
          clearInterval(heartbeatTimer);
        } else {
          // 期限で外れただけ（裏のタブから戻ったときなど）: 取り直す
          await acquire();
        }
      } else if ((status.value === "held" || status.value === "free") && !s.holder) {
        // 空いたら、編集できる人はロックを取る（引き継がれた人は自分で引き継ぎ直す）
        if (canEdit()) await acquire();
        else status.value = "free";
      } else if (status.value === "free" && s.holder) {
        status.value = "held";
      }
    } catch (e) {
      error.value = e;
    }
  }

  async function takeOver(): Promise<boolean> {
    error.value = null;
    try {
      const got = await api.post<{ lockId: string; heartbeatSeconds: number }>(`${base}/take-over`);
      lockId.value = got.lockId;
      status.value = "mine";
      startHeartbeat(got.heartbeatSeconds);
      await readState();
      return true;
    } catch (e) {
      error.value = e;
      return false;
    }
  }

  /** ロックを外す（画面を離れるとき。ページを閉じても届くよう keepalive で送る）。 */
  function release() {
    clearInterval(heartbeatTimer);
    clearInterval(viewerTimer);
    if (!lockId.value) return;
    const token = csrfToken();
    void fetch(`/api/admin${base}/${lockId.value}`, {
      method: "DELETE",
      keepalive: true,
      credentials: "same-origin",
      headers: token ? { "X-CSRF-Token": token } : {},
    }).catch(() => {});
    lockId.value = null;
  }

  const onVisibility = () => {
    if (document.hidden) {
      hiddenSince = Date.now();
    } else {
      hiddenSince = null;
      void refresh();
    }
  };

  useRealtime((event) => {
    if (event.type === "reconnected" || ("entryId" in event && event.type === "editLock" && event.entryId === entryId)) {
      void refresh();
    }
  });

  onMounted(async () => {
    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("pagehide", release);
    if (canEdit()) {
      await acquire();
    } else {
      const s = await readState().catch(() => null);
      status.value = s?.holder ? "held" : "free";
    }
    // 期限で外れたことは知らせが来ないので、閲覧のみの間はときどき確かめる
    viewerTimer = setInterval(() => {
      if (status.value === "held" || status.value === "free") void refresh();
    }, VIEWER_REFRESH_MS);
  });
  onBeforeUnmount(() => {
    document.removeEventListener("visibilitychange", onVisibility);
    window.removeEventListener("pagehide", release);
    release();
  });

  return { lockId, status, holder, since, error, takeOver, refresh };
}
