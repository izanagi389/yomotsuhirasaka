// 画面の隅に短く出す知らせ（トースト）。「保存しました」など、操作の結果を伝えるだけに使う。
// 依存を増やさず自作する（持ち主の判断）。数秒で消え、閉じるボタンでも消せる。読み上げは aria-live で行う。

export type ToastKind = "success" | "error" | "info";
export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
}

const DURATION_MS = 4000;
let next = 1;

export function useToast() {
  const toasts = useState<Toast[]>("toasts", () => []);

  function dismiss(id: number) {
    toasts.value = toasts.value.filter((t) => t.id !== id);
  }

  function show(message: string, kind: ToastKind = "success") {
    const id = next++;
    toasts.value = [...toasts.value.slice(-2), { id, kind, message }];
    setTimeout(() => dismiss(id), kind === "error" ? DURATION_MS * 2 : DURATION_MS);
  }

  return { toasts, show, dismiss };
}
