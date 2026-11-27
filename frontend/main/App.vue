<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, provide, ref, watch } from "vue";
import TitleBar from "../components/TitleBar.vue";
import Icon from "../components/Icon.vue";
import ToastHost from "../components/ToastHost.vue";
import DownloadList from "./DownloadList.vue";
import TransferView from "./TransferView.vue";
import TranslateView from "./TranslateView.vue";
import ScreenshotView from "./ScreenshotView.vue";
import TransferProgress from "./TransferProgress.vue";
import IncomingConfirm from "./IncomingConfirm.vue";
import { useToast } from "../composables/useToast.js";
import { t } from "../i18n/index.js";
import { useDownloads } from "./useDownloads.js";
import { fmtSpeed } from "./format.js";
import { useTransfer } from "./useTransfer.js";
import { MAIN_TOOLS, useTools } from "./useTools.js";

const toast = useToast();
const downloads = useDownloads();
const tools = useTools();

const view = ref("download");
const sub = ref("send");
const tb = ref(null);

/* 当前工具被停用（或深链指向未启用工具）时落到第一个可见工具；
 * 全部停用时保留当前页，让用户仍能看到已打开的内容。 */
function ensureVisible() {
  if (!tools.visible.value.length) return;
  if (!tools.isVisible(view.value)) view.value = tools.visible.value[0].id;
}

function setView(next) {
  if (!MAIN_TOOLS.some((tool) => tool.id === next)) return;
  if (!tools.isVisible(next)) {
    ensureVisible();
    return;
  }
  view.value = next;
  if (next === "transfer") {
    transfer.onSub(sub.value);
  }
}

function setSub(next) {
  sub.value = next;
  transfer.onSub(next);
}

const transfer = useTransfer({ toast, setView, setSub });

provide("downloads", downloads);
provide("transfer", transfer);

const speedText = computed(() =>
  t("main.sideSpeed", {
    down: fmtSpeed(downloads.global.value.download_speed || 0),
    up: fmtSpeed(downloads.global.value.upload_speed || 0),
  }),
);

const title = computed(() => {
  if (transfer.tpOpen.value) return t("main.titleTransferProgress");
  if (!tools.visible.value.length) return t("main.noToolsTitle");
  if (view.value === "transfer") return t("main.titleTransfer");
  if (view.value === "translate") return t("main.navTranslate");
  if (view.value === "screenshot") return t("main.navScreenshot");
  if (downloads.filter.value === "github") return t("main.titleGithub");
  return t("main.titleTasks");
});

watch(
  () => downloads.filter.value,
  (cat) => {
    if (cat === "github") transfer.closeHistoryMenu();
  },
);

let unlistenSwitch = null;

function switchTo(next) {
  if (typeof next !== "string") return;
  if (!MAIN_TOOLS.some((tool) => tool.id === next)) return;
  if (!tools.isVisible(next)) return;
  setView(next);
  if (next === "transfer") setSub("send");
}

function openSettings() {
  window.__TAURI__.core
    .invoke("open_settings_cmd")
    .catch((e) => toast.show(String(e && e.message ? e.message : e), { kind: "error" }));
}

function openToolSettings() {
  window.__TAURI__.core
    .invoke("open_tool_settings")
    .catch((e) => toast.show(String(e && e.message ? e.message : e), { kind: "error" }));
}

async function onFocus() {
  await tools.refresh();
  ensureVisible();
}

onMounted(async () => {
  document.addEventListener("click", transfer.closeHistoryMenu);
  window.addEventListener("focus", onFocus);
  await tools.refresh();
  ensureVisible();
  await downloads.init();
  // 传输模块的首次拉取可能较慢，不让它拖住首帧显示（legacy 也是异步初始化）。
  transfer.init();
  unlistenSwitch = await window.__TAURI__.event.listen("main:switch-view", (ev) => {
    switchTo(ev.payload);
  });
  await nextTick();
  tb.value?.reveal();
});

onBeforeUnmount(() => {
  window.removeEventListener("focus", onFocus);
  if (unlistenSwitch) unlistenSwitch();
});
</script>

<template>
  <div class="win">
    <TitleBar ref="tb" :title="title">
      <template #actions>
        <button
          type="button"
          class="tb-btn"
          :title="$t('common.openSettings')"
          :aria-label="$t('common.openSettings')"
          @click="openSettings"
        >
          <Icon name="gear" :size="14" />
        </button>
      </template>
    </TitleBar>

    <div class="app">
      <aside class="side">
        <nav class="nav">
          <button
            v-for="tool in tools.visible.value"
            :key="tool.id"
            class="nav-item"
            :class="{ active: view === tool.id }"
            type="button"
            @click="setView(tool.id)"
          >
            <span class="nav-ico"><Icon :name="tool.icon" :size="16" /></span>
            <span class="nav-label">{{ $t(tool.labelKey) }}</span>
            <span
              v-if="tool.id === 'download'"
              class="nav-badge"
              :class="{ hidden: !downloads.counts.value.running }"
              :title="$t('main.runningCount')"
            >
              {{ downloads.counts.value.running }}
            </span>
          </button>
        </nav>

        <div class="side-foot">
          <span id="global" class="num">{{ speedText }}</span>
        </div>
      </aside>

      <div v-if="!tools.visible.value.length" id="noTools" class="main">
        <header class="head">
          <h1>{{ $t("main.noToolsTitle") }}</h1>
        </header>
        <div class="page-body">
          <div class="empty no-tools">
            <span class="empty-ico"><Icon name="gear" :size="30" /></span>
            <span>{{ $t("main.noToolsBody") }}</span>
            <button class="btn btn-primary" type="button" @click="openToolSettings">
              {{ $t("main.noToolsAction") }}
            </button>
          </div>
        </div>
      </div>

      <template v-else>
        <DownloadList v-show="view === 'download'" />
        <TransferView v-show="view === 'transfer'" :sub="sub" @update:sub="setSub" />
        <TranslateView v-show="view === 'translate'" />
        <ScreenshotView v-show="view === 'screenshot'" />
      </template>
    </div>

    <TransferProgress />
    <IncomingConfirm />
    <ToastHost />
  </div>
</template>

<style scoped>
.no-tools {
  flex: 1 1 auto;
}
</style>
