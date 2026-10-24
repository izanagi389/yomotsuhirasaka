// 下書きの自動保存（03-content-and-editing.md「同時編集」、Q73, Q137）。
//
// - 変えたフィールドだけを、入力が止まってから数秒後に保存する。保存には、最後に読んだ（保存した）リビジョン番号を付ける。
// - 他の人が先に保存していれば、サーバーは 409（revision_conflict）を返して何も書かない。そのときは自動保存を止め、
//   画面に知らせる。未保存の変更は画面に残す（再読み込みすると消える）。
// - 一度に送る保存は1つだけ。送っている間の変更は、終わってから続けて送る。

import type { Draft, FieldValue } from "~/utils/content";

export type SaveStatus = "saved" | "dirty" | "saving" | "conflict" | "locked" | "error";

const DELAY_MS = 2000;
const RETRY_MS = 10000;

export function useAutosave(save: (revision: number, fields: Record<string, FieldValue>) => Promise<Draft>, initialRevision: number) {
  const revision = ref(initialRevision);
  const status = ref<SaveStatus>("saved");
  const error = ref<unknown>(null);
  const pending = new Map<string, FieldValue>();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let inFlight: Promise<void> | null = null;

  function schedule(ms = DELAY_MS) {
    clearTimeout(timer);
    timer = setTimeout(() => void flush(), ms);
  }

  /** フィールドの値が変わった。 */
  function set(name: string, value: FieldValue) {
    pending.set(name, value);
    if (status.value === "conflict" || status.value === "locked") return;
    status.value = inFlight ? "saving" : "dirty";
    schedule();
  }

  async function run(): Promise<void> {
    const fields = Object.fromEntries(pending);
    pending.clear();
    status.value = "saving";
    error.value = null;
    try {
      const draft = await save(revision.value, fields);
      revision.value = draft.revision;
      status.value = pending.size > 0 ? "dirty" : "saved";
    } catch (e) {
      // 送れなかった値を戻す（その後に変えた値のほうを残す）
      for (const [k, v] of Object.entries(fields)) if (!pending.has(k)) pending.set(k, v);
      error.value = e;
      if (e instanceof ApiError && e.code === "revision_conflict") {
        status.value = "conflict";
        clearTimeout(timer);
        return;
      }
      // ほかの画面が編集中（編集を引き継がれた）: 止めて、入力は画面に残す（Q73）
      if (e instanceof ApiError && e.code === "entry_locked") {
        status.value = "locked";
        clearTimeout(timer);
        return;
      }
      status.value = "error";
      // 入力の誤り（422 など）は、次の変更を待つ。つながらないときは、しばらくして送り直す
      if (!(e instanceof ApiError)) schedule(RETRY_MS);
    }
  }

  /** 未保存の変更をすぐに保存し、終わるのを待つ（公開の前など）。保存できなければ false。 */
  async function flush(): Promise<boolean> {
    clearTimeout(timer);
    while (inFlight) await inFlight;
    if (status.value === "conflict" || status.value === "locked") return false;
    if (pending.size > 0) {
      inFlight = run().finally(() => {
        inFlight = null;
      });
      await inFlight;
      if (pending.size > 0 && status.value === "dirty") return flush();
    }
    return status.value === "saved";
  }

  /** サーバーから読み直したときに、リビジョン番号と状態を戻す。 */
  function reset(newRevision: number) {
    clearTimeout(timer);
    pending.clear();
    revision.value = newRevision;
    status.value = "saved";
    error.value = null;
  }

  const hasUnsaved = computed(() => status.value !== "saved");

  // 未保存のまま閉じようとしたら確かめる
  const onBeforeUnload = (ev: BeforeUnloadEvent) => {
    if (hasUnsaved.value) ev.preventDefault();
  };
  onMounted(() => window.addEventListener("beforeunload", onBeforeUnload));
  onBeforeUnmount(() => {
    window.removeEventListener("beforeunload", onBeforeUnload);
    clearTimeout(timer);
  });

  /** 編集を引き継ぎ直したあと、止めていた保存を再び始める（入力は残っている）。 */
  function unlock() {
    if (status.value !== "locked") return;
    status.value = pending.size > 0 ? "dirty" : "saved";
    if (pending.size > 0) schedule(0);
  }

  return { revision, status, error, set, flush, reset, unlock, hasUnsaved };
}
