<script setup>
import { computed, inject } from "vue";
import Icon from "../components/Icon.vue";
import { t } from "../i18n/index.js";

const transfer = inject("transfer");

const req = computed(() => transfer.cfReq.value);
const files = computed(() => (req.value && req.value.files) || []);
const shownFiles = computed(() => files.value.slice(0, 4));
const countText = computed(() =>
  files.value.length === 1
    ? t("main.cfOneFile")
    : t("main.cfNFiles", { n: files.value.length }),
);
const previewText = computed(() =>
  files.value.length === 1
    ? files.value[0].name
    : t("main.cfViewList", { n: files.value.length }),
);

function openDir() {
  if (!transfer.receiveDir.value) return;
  window.__TAURI__.core
    .invoke("open_path", { path: transfer.receiveDir.value })
    .catch((e) => (transfer.cfError.value = String(e && e.message ? e.message : e)));
}
</script>

<template>
  <div id="cfLayer" class="cf-layer" v-show="req">
    <div class="cf-dialog" role="dialog" aria-modal="true" aria-labelledby="cfSender">
      <button
        class="icon-btn cf-close"
        type="button"
        :title="$t('main.cfRejectClose')"
        :aria-label="$t('main.cfRejectClose')"
        :disabled="transfer.cfBusy.value"
        @click="transfer.respondIncoming(false)"
      >
        <Icon name="close" :size="14" />
      </button>

      <div class="cf-icon"><Icon :name="transfer.deviceIcon(req && req.sender_type)" :size="20" /></div>
      <div class="cf-alias" id="cfSender">{{ (req && req.sender_alias) || $t("main.unknownDevice") }}</div>

      <div class="cf-tags">
        <div class="cf-tag cf-ip" :title="(req && req.sender_ip) || ''">
          {{ transfer.shortIp(req && req.sender_ip) }}
        </div>
        <div class="cf-tag">{{ (req && req.sender_model) || $t("main.unknownModel") }}</div>
      </div>

      <div class="cf-prompt">
        {{ $t("main.cfPrompt") }} <b>{{ countText }}</b>
      </div>

      <div class="cf-file">
        <div class="cf-file-preview">
          <span class="ico"><Icon name="file" :size="14" /></span>
          <span class="fname">{{ previewText }}</span>
        </div>
        <div class="cf-tooltip">
          <ul>
            <li v-for="(f, i) in shownFiles" :key="f.name + i">
              <span class="ico"><Icon name="file" :size="14" /></span>
              <span class="n" :title="f.name">{{ f.name }}</span>
            </li>
            <li v-if="files.length > shownFiles.length" class="more">……</li>
          </ul>
        </div>
      </div>
      <div class="cf-total">{{ $t("main.cfTotal", { size: transfer.fmtBytes(req && req.total_bytes) }) }}</div>

      <div class="cf-path">
        <div class="path-text" :title="transfer.receiveDir.value || ''">
          {{ transfer.receiveDir.value || "—" }}
        </div>
        <button
          class="icon-btn cf-folder"
          type="button"
          :title="$t('main.openReceiveDir')"
          :aria-label="$t('main.openReceiveDir')"
          @click="openDir"
        >
          <Icon name="folder" :size="16" />
        </button>
      </div>

      <div class="notice notice-danger" v-show="transfer.cfError.value">
        {{ transfer.cfError.value }}
      </div>

      <div class="cf-actions">
        <button
          class="btn cf-reject"
          type="button"
          :disabled="transfer.cfBusy.value"
          @click="transfer.respondIncoming(false)"
        >
          {{ $t("main.cfReject") }}
        </button>
        <button
          class="btn btn-primary"
          type="button"
          :disabled="transfer.cfBusy.value"
          @click="transfer.respondIncoming(true)"
        >
          {{ $t("main.cfAccept") }}
        </button>
      </div>
    </div>
  </div>
</template>
