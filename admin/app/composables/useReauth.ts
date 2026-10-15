// 再認証（重い操作の前。13-security.md、Q108）。ダイアログ（components/ReauthDialog.vue）を開き、結果を待つ。

export function useReauth() {
  const open = useState<boolean>("reauth:open", () => false);
  const waiting = useState<((ok: boolean) => void)[]>("reauth:waiting", () => []);

  /** ダイアログを開き、再認証が通ったら true。 */
  function request(): Promise<boolean> {
    open.value = true;
    return new Promise((resolve) => {
      waiting.value.push(resolve);
    });
  }

  function finish(ok: boolean): void {
    open.value = false;
    const all = waiting.value;
    waiting.value = [];
    for (const resolve of all) resolve(ok);
  }

  return { open, request, finish };
}
