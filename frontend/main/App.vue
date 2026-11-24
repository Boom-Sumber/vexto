<script setup>
import { computed, nextTick, onMounted, provide, ref, watch } from "vue";
import TitleBar from "../components/TitleBar.vue";
import Icon from "../components/Icon.vue";
import ToastHost from "../components/ToastHost.vue";
import DownloadList from "./DownloadList.vue";
import TransferView from "./TransferView.vue";
import TransferProgress from "./TransferProgress.vue";
import IncomingConfirm from "./IncomingConfirm.vue";
import { useToast } from "../composables/useToast.js";
import { t } from "../i18n/index.js";
import { useDownloads } from "./useDownloads.js";
import { fmtSpeed } from "./format.js";
import { useTransfer } from "./useTransfer.js";

const toast = useToast();
const downloads = useDownloads();

const view = ref("download");
const sub = ref("send");
const tb = ref(null);

function setView(next) {
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

const CATS = [
  ["all", "main.catAll"],
  ["running", "main.catRunning"],
  ["failed", "main.catFailed"],
  ["complete", "main.catComplete"],
  ["deleted", "main.catDeleted"],
];

const SUBS = [
  ["send", "main.subSend"],
  ["receive", "main.subReceive"],
  ["history", "main.subHistory"],
];

const speedText = computed(() =>
  t("main.sideSpeed", {
    down: fmtSpeed(downloads.global.value.download_speed || 0),
    up: fmtSpeed(downloads.global.value.upload_speed || 0),
  }),
);

const title = computed(() => {
  if (transfer.tpOpen.value) return t("main.titleTransferProgress");
  if (view.value === "transfer") return t("main.titleTransfer");
  if (downloads.filter.value === "github") return t("main.titleGithub");
  return t("main.titleTasks");
});

function selectCat(cat) {
  downloads.setFilter(cat);
}

function selectView(next) {
  setView(next);
}

function selectSub(next) {
  setSub(next);
}

watch(
  () => downloads.filter.value,
  (cat) => {
    if (cat === "github") transfer.closeHistoryMenu();
  },
);

onMounted(async () => {
  document.addEventListener("click", transfer.closeHistoryMenu);
  await downloads.init();
  // 传输模块的首次拉取可能较慢，不让它拖住首帧显示（legacy 也是异步初始化）。
  transfer.init();
  await nextTick();
  tb.value?.reveal();
});
</script>

<template>
  <div class="win">
    <TitleBar ref="tb" :title="title" />

    <div class="app">
      <aside class="side">
        <div class="brand">
          <span class="logo logo-badge" aria-hidden="true"></span>
          <span class="brand-name">Vexto</span>
        </div>

        <nav class="nav">
          <button
            class="nav-item"
            :class="{ active: view === 'download' }"
            type="button"
            @click="selectView('download')"
          >
            <span class="nav-ico"><Icon name="download" :size="16" /></span>
            <span class="nav-label">{{ $t("main.navDownload") }}</span>
            <span
              class="nav-badge"
              :class="{ hidden: !downloads.counts.value.running }"
              :title="$t('main.runningCount')"
            >
              {{ downloads.counts.value.running }}
            </span>
          </button>
          <button
            v-if="transfer.enabled.value"
            class="nav-item"
            :class="{ active: view === 'transfer' }"
            type="button"
            @click="selectView('transfer')"
          >
            <span class="nav-ico"><Icon name="transfer" :size="16" /></span>
            <span class="nav-label">{{ $t("main.navTransfer") }}</span>
          </button>
        </nav>

        <div class="subnav" id="catsWrap" v-show="view === 'download'">
          <div class="sub-title">{{ $t("main.subTitleCategory") }}</div>
          <nav class="cats">
            <button
              v-for="[key, label] in CATS"
              :key="key"
              class="cat"
              :class="{ active: downloads.filter.value === key }"
              type="button"
              @click="selectCat(key)"
            >
              {{ $t(label) }} <span>{{ downloads.counts.value[key] }}</span>
            </button>
            <div class="cat-sep" role="separator"></div>
            <button
              class="cat cat-github"
              :class="{ active: downloads.filter.value === 'github' }"
              type="button"
              @click="selectCat('github')"
            >
              {{ $t("main.catGithub") }}
            </button>
          </nav>
        </div>

        <div class="subnav" id="subTransfer" v-show="view === 'transfer'">
          <div class="sub-title">{{ $t("main.subTitleTransfer") }}</div>
          <button
            v-for="[key, label] in SUBS"
            :key="key"
            class="sub-item"
            :class="{ active: sub === key }"
            type="button"
            @click="selectSub(key)"
          >
            {{ $t(label) }}
          </button>
        </div>

        <div class="side-foot">
          <span id="global" class="num">{{ speedText }}</span>
        </div>
      </aside>

      <DownloadList v-show="view === 'download'" />
      <TransferView v-show="view === 'transfer'" :sub="sub" />
    </div>

    <TransferProgress />
    <IncomingConfirm />
    <ToastHost />
  </div>
</template>
