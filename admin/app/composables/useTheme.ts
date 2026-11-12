// 画面の明るさ（OS に従う・明るい・暗い）。選んだものはこのブラウザにだけ覚える（サーバーには送らない）。
// 覚えられない環境（保存が止められているなど）では、OS に従う。

export type ThemePreference = "system" | "light" | "dark";
const KEY = "yomi:theme";

function stored(): ThemePreference {
  try {
    const v = localStorage.getItem(KEY);
    return v === "light" || v === "dark" ? v : "system";
  } catch {
    return "system";
  }
}

function systemDark(): boolean {
  return typeof window !== "undefined" && window.matchMedia("(prefers-color-scheme: dark)").matches;
}

/** <html> に data-theme を付ける（CSS の暗い色はこれで切り替わる）。 */
export function applyTheme(preference: ThemePreference) {
  const dark = preference === "dark" || (preference === "system" && systemDark());
  document.documentElement.dataset.theme = dark ? "dark" : "light";
}

export function useTheme() {
  const preference = useState<ThemePreference>("theme", () => (import.meta.client ? stored() : "system"));

  function setPreference(p: ThemePreference) {
    preference.value = p;
    try {
      if (p === "system") localStorage.removeItem(KEY);
      else localStorage.setItem(KEY, p);
    } catch {
      // 覚えられなくても、この画面には反映する
    }
    applyTheme(p);
  }

  return { preference, setPreference };
}
