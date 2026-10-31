// 編集者向けのAI支援（矛盾の検知・見直しの手がかり・構成案。M4）の、始めて結果を待つ操作。
// - start() は POST で始め（同じ下書き・候補の結果があれば、サーバーはお金をかけずにそれを返す）、running の間は GET で読み直す。
// - 画面を離れたら読み直しをやめる。結果の状態（予算切れなど）の判断はサーバーが行う。
import type { AssistState } from "~/utils/content";

/** running の間に読み直す間隔（ミリ秒）。 */
const POLL_MS = 2000;

export function useAssistTask<T extends { state: AssistState }>(path: () => string) {
  const api = useApi();
  const result = ref<T | null>(null) as Ref<T | null>;
  const error = ref<unknown>(null);
  let timer: ReturnType<typeof setTimeout> | null = null;
  let stopped = false;

  function stop() {
    stopped = true;
    if (timer) clearTimeout(timer);
    timer = null;
  }
  function schedule() {
    if (stopped || result.value?.state !== "running") return;
    timer = setTimeout(async () => {
      try {
        result.value = await api.get<T>(path());
      } catch (e) {
        error.value = e;
        return;
      }
      schedule();
    }, POLL_MS);
  }
  async function load() {
    stopped = false;
    error.value = null;
    try {
      result.value = await api.get<T>(path());
    } catch (e) {
      error.value = e;
    }
    schedule();
  }
  async function start() {
    stop();
    stopped = false;
    error.value = null;
    try {
      result.value = await api.post<T>(path());
    } catch (e) {
      error.value = e;
      return;
    }
    schedule();
  }
  onBeforeUnmount(stop);
  return { result, error, start, load, stop };
}
