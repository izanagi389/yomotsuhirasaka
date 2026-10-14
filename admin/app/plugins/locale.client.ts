// ログインした本人の言語の設定があれば、それに合わせる（なければブラウザの言語）。
export default defineNuxtPlugin(() => {
  const { me } = useAuth();
  const i18n = useNuxtApp().$i18n;
  watch(
    () => me.value?.locale,
    (locale) => {
      if (locale && i18n.locale.value !== locale) i18n.setLocale(locale);
    },
    { immediate: true },
  );
});
