<script setup>
import { computed } from "vue";
import { t } from "../i18n/index.js";
import { fmtBytes } from "./utils.js";

const props = defineProps({
  update: { type: Object, required: true },
});

const s = computed(() => props.update.state.value);
const info = computed(() => props.update.info.value);
const phase = computed(() => (s.value ? s.value.phase : "idle"));

const statusText = computed(() => {
  const v = s.value;
  if (!v) return t("settings.updateLoading");
  switch (v.phase) {
    case "idle":
      return v.lastCheck ? t("settings.updateIdleHint") : t("settings.updateIdleNever");
    case "checking":
      return t("settings.updateChecking");
    case "available":
      return t("settings.updateAvailable", { version: v.latest || "" });
    case "up_to_date":
      return t("settings.updateUpToDate");
    case "downloading": {
      const total = v.assetSize || 0;
      const got = v.downloaded || 0;
      const pct = total > 0 ? Math.floor((got / total) * 100) : 0;
      return total > 0
        ? t("settings.updateDownloadingTotal", {
            got: fmtBytes(got),
            total: fmtBytes(total),
            pct,
          })
        : t("settings.updateDownloading", { got: fmtBytes(got) });
    }
    case "ready":
      return t("settings.updateReady");
    case "installing":
      return t("settings.updateInstalling");
    case "error":
      return v.error || t("settings.updateError");
    default:
      return "";
  }
});

const pct = computed(() => {
  const v = s.value;
  if (!v || v.phase !== "downloading") return 0;
  const total = v.assetSize || 0;
  return total > 0 ? Math.min(100, Math.round(((v.downloaded || 0) / total) * 100)) : 0;
});

function run(cmd, btn) {
  props.update.run(cmd, btn);
}
</script>

<template>
  <div class="about-update-block">
    <div class="upd-state" :class="{ bad: s && s.phase === 'error' }">{{ statusText }}</div>

    <div v-if="s && s.phase === 'downloading'" class="upd-bar">
      <i :style="{ width: pct + '%' }"></i>
    </div>

    <div v-if="s && s.required && s.minVersion" class="upd-required">
      {{ $t("settings.updateRequired", { version: s.minVersion }) }}
    </div>

    <div class="about-actions">
      <button
        v-if="phase === 'checking'"
        class="btn btn-primary"
        type="button"
        disabled
      >
        {{ $t("settings.updateCheckingBtn") }}
      </button>
      <button
        v-else-if="phase === 'available'"
        class="btn btn-primary"
        type="button"
        @click="run(info.update.download, $event.currentTarget)"
      >
        {{ $t("settings.updateDownloadInstall") }}
      </button>
      <button
        v-else-if="phase === 'downloading'"
        class="btn"
        type="button"
        @click="run(info.update.cancel, $event.currentTarget)"
      >
        {{ $t("settings.updateCancelDownload") }}
      </button>
      <button
        v-else-if="phase === 'ready'"
        class="btn btn-primary"
        type="button"
        @click="update.requestInstall(info.update.install)"
      >
        {{ $t("settings.updateInstallNow") }}
      </button>
      <button
        v-else-if="phase === 'installing'"
        class="btn btn-primary"
        type="button"
        disabled
      >
        {{ $t("settings.updateInstallingBtn") }}
      </button>
      <button
        v-else-if="phase === 'error'"
        class="btn btn-primary"
        type="button"
        @click="run(info.update.check, $event.currentTarget)"
      >
        {{ $t("settings.updateRetry") }}
      </button>
      <button
        v-else
        class="btn btn-primary"
        type="button"
        @click="run(info.update.check, $event.currentTarget)"
      >
        {{ $t("settings.updateCheck") }}
      </button>

      <button class="btn" type="button" @click="update.copyDiagnostics($event.currentTarget)">
        {{ $t("settings.updateCopyDiag") }}
      </button>
      <button class="btn" type="button" @click="update.openDir()">
        {{ $t("settings.updateOpenLogs") }}
      </button>
    </div>

    <details
      v-if="s && s.notes && ['available', 'downloading', 'ready'].includes(s.phase)"
      class="about-update-notes"
    >
      <summary>{{ $t("settings.updateNotes") }}</summary>
      <div class="upd-notes">{{ s.notes }}</div>
    </details>
  </div>
</template>
