<script setup lang="ts">
// 記事の公開設定のスイッチ（Q67, Q184）。初期値から変えていないものは折りたたむ。
// M1 では保存と変更の Webhook だけで、効果は M2 以降。変えられるのは公開できる人（管理者・編集者とサーバー管理者）。
import { Switch } from "@/components/ui/switch";
import { DEFAULT_PUBLISH_SETTINGS, type PublishSettings } from "~/utils/content";

const props = defineProps<{ workspaceId: string; entryId: string; editable: boolean }>();
const { t } = useI18n();
const api = useApi();
const path = computed(() => `/workspaces/${props.workspaceId}/entries/${props.entryId}/publish-settings`);

const { data: settings, error } = useAsyncData(`settings:${props.entryId}`, () => api.get<PublishSettings>(path.value));
const keys = Object.keys(DEFAULT_PUBLISH_SETTINGS) as (keyof PublishSettings)[];
const changed = computed(() => keys.filter((k) => settings.value && settings.value[k] !== DEFAULT_PUBLISH_SETTINGS[k]));
const unchanged = computed(() => keys.filter((k) => !changed.value.includes(k)));
const showDefaults = ref(false);

const saving = ref(false);
const saveError = ref<unknown>(null);
async function set(key: keyof PublishSettings, value: boolean) {
  saving.value = true;
  saveError.value = null;
  try {
    settings.value = await api.patch<PublishSettings>(path.value, { [key]: value });
  } catch (e) {
    saveError.value = e;
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <div class="grid gap-4 text-sm">
    <FormError :error="error ?? saveError" />
    <p class="text-muted-foreground">{{ t("settings.note") }}</p>
    <template v-if="settings">
      <ul class="grid gap-3">
        <li v-for="k in changed" :key="k" class="flex items-start gap-3">
          <Switch :id="`setting-${k}`" :model-value="settings[k]" :disabled="!editable || saving" @update:model-value="(v) => set(k, v as boolean)" />
          <label :for="`setting-${k}`" class="grid gap-0.5">
            <span class="font-medium">{{ t(`settings.${k}.label`) }}</span>
            <span class="text-muted-foreground">{{ t(`settings.${k}.help`) }}</span>
            <span class="text-xs text-amber-700 dark:text-amber-400">{{ t("settings.changedFromDefault") }}</span>
          </label>
        </li>
      </ul>
      <button v-if="unchanged.length > 0" type="button" class="w-fit text-muted-foreground underline" :aria-expanded="showDefaults" @click="showDefaults = !showDefaults">
        {{ showDefaults ? t("settings.hideDefaults") : t("settings.showDefaults", { n: unchanged.length }) }}
      </button>
      <ul v-if="showDefaults" class="grid gap-3">
        <li v-for="k in unchanged" :key="k" class="flex items-start gap-3">
          <Switch :id="`setting-${k}`" :model-value="settings[k]" :disabled="!editable || saving" @update:model-value="(v) => set(k, v as boolean)" />
          <label :for="`setting-${k}`" class="grid gap-0.5">
            <span class="font-medium">{{ t(`settings.${k}.label`) }}</span>
            <span class="text-muted-foreground">{{ t(`settings.${k}.help`) }}</span>
          </label>
        </li>
      </ul>
    </template>
  </div>
</template>
