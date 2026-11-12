// 起動時に画面の明るさを決め、「OS に従う」のときは OS の切り替えにもついていく。
export default defineNuxtPlugin(() => {
  const { preference } = useTheme();
  applyTheme(preference.value);
  window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
    if (preference.value === "system") applyTheme("system");
  });
});
