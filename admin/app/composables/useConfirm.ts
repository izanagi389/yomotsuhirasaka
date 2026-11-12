// 取り消せない操作の前の確認。`if (!(await confirm({ ... }))) return;` の形で使う。
// ダイアログは レイアウトの ConfirmHost が1つだけ出す（見た目と文言の型をそろえるため）。
// 確認のダイアログを閉じてから操作を始めるので、操作の失敗はそれぞれの画面に出す。

export interface ConfirmOptions {
  title: string;
  description: string;
  confirmLabel: string;
  /** 消す・取り消すなど元に戻せない操作。確定のボタンを赤にする。 */
  destructive?: boolean;
}

interface ConfirmState extends ConfirmOptions {
  open: boolean;
}

let resolver: ((ok: boolean) => void) | null = null;

export function useConfirm() {
  const state = useState<ConfirmState>("confirm", () => ({ open: false, title: "", description: "", confirmLabel: "" }));

  function confirm(options: ConfirmOptions): Promise<boolean> {
    resolver?.(false);
    state.value = { ...options, open: true };
    return new Promise((resolve) => {
      resolver = resolve;
    });
  }

  function settle(ok: boolean) {
    state.value = { ...state.value, open: false };
    const r = resolver;
    resolver = null;
    r?.(ok);
  }

  return { state, confirm, settle };
}
