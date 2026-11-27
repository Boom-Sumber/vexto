<script setup>
import { computed, inject } from "vue";
import Icon from "../components/Icon.vue";
import SegmentedControl from "../components/SegmentedControl.vue";
import Tabs from "../components/Tabs.vue";
import { useToast } from "../composables/useToast.js";
import { t } from "../i18n/index.js";

defineProps({ sub: { type: String, default: "send" } });
defineEmits(["update:sub"]);

const SUBS = [
  ["send", "main.subSend"],
  ["receive", "main.subReceive"],
  ["history", "main.subHistory"],
];

const subItems = computed(() => SUBS.map(([id, key]) => ({ id, label: t(key) })));

const transfer = inject("transfer");
const toast = useToast();

/* 历史分页：3 档，用分段控件（和设置页的选择控件同一套视觉） */
const PAGE_SIZES = [
  [10, "10"],
  [20, "20"],
  [50, "50"],
];

function setPageSize(v) {
  transfer.hs.pageSize = parseInt(v, 10) || 20;
  transfer.hsLoad(1);
}

const pickCount = computed(() => transfer.pickPaths.value.length);
const pickSize = computed(() =>
  transfer.pickSizes.value.reduce((a, b) => a + (b || 0), 0),
);

function baseName(path) {
  return path.replace(/^.*[\\/]/, "");
}

function openDir() {
  if (!transfer.receiveDir.value) {
    toast.show(t("main.noReceiveDir"), { kind: "error" });
    return;
  }
  window.__TAURI__.core
    .invoke("open_path", { path: transfer.receiveDir.value })
    .catch((e) => toast.show(String(e && e.message ? e.message : e), { kind: "error" }));
}

function menuEntries(item) {
  const first = (item.files || [])[0] || {};
  const hasPath = !!(first.path && first.path.length);
  return [
    { key: "open", icon: "external", label: t("main.openFile"), disabled: !hasPath },
    { key: "reveal", icon: "folder", label: t("main.openFolder"), disabled: !hasPath },
    { key: "info", icon: "info", label: t("main.fileInfo"), disabled: false },
    { key: "del", icon: "trash", label: t("main.delHistory"), disabled: false, danger: true },
  ];
}

function onMenuEntry(item, entry) {
  transfer.closeHistoryMenu();
  if (entry.disabled) return;
  if (entry.key === "open") transfer.openHistoryPath(item, false);
  else if (entry.key === "reveal") transfer.openHistoryPath(item, true);
  else if (entry.key === "info") transfer.showHistoryInfo(item);
  else if (entry.key === "del") transfer.hsRemove(item);
}

function infoRows(item) {
  const rows = [];
  const files = item.files || [];
  rows.push([t("main.hsDirection"), item.direction === "send" ? t("main.hsSend") : t("main.hsReceive")]);
  rows.push([t("main.hsPeer"), item.peer || "—"]);
  rows.push([t("main.hsResult"), transfer.hsStatusText(item.status)]);
  rows.push([t("main.hsTime"), transfer.fmtWhen(item.at_ms)]);
  rows.push([t("main.hsTotal"), transfer.fmtBytes(item.total_bytes || 0)]);
  if (files.length > 1) rows.push([t("main.hsFileCount"), t("main.hsFiles", { n: files.length })]);
  for (const f of files) {
    rows.push([t("main.hsFileName"), f.name || "—"]);
    rows.push([t("main.hsFileSize"), transfer.fmtBytes(f.size || 0)]);
    rows.push([t("main.hsPath"), f.path || t("main.hsNoPath")]);
  }
  const paths = files.map((f) => f.path).filter(Boolean);
  if (item.message && !paths.includes(item.message)) {
    rows.push([t("main.hsNote"), item.message]);
  }
  return rows;
}

function pages() {
  return Math.max(1, Math.ceil(transfer.hs.total / transfer.hs.pageSize));
}

function firstFileName(item) {
  const names = (item.files || []).map((f) => f.name).filter(Boolean);
  const first = names[0] || t("main.hsSession");
  return names.length > 1
    ? t("main.hsMoreFiles", { name: first, n: names.length })
    : first;
}

function subText(item) {
  const sending = item.direction === "send";
  return [
    transfer.fmtBytes(item.total_bytes || 0),
    sending
      ? t("main.hsTo", { peer: item.peer || t("main.peerFallback") })
      : t("main.hsFrom", { peer: item.peer || t("main.peerFallback") }),
    transfer.fmtWhen(item.at_ms),
  ].join("　·　");
}
</script>

<template>
  <div id="transferView" class="main">
    <div class="view-tabs" id="subTransfer">
      <Tabs :model-value="sub" :items="subItems" @update:model-value="$emit('update:sub', $event)" />
    </div>

    <!-- 发送 -->
    <div v-show="sub === 'send'" id="paneSend" class="pane">
      <header class="head">
        <h1>{{ $t("main.transferTitle") }}</h1>
        <div class="tools">
          <button id="tfRefresh" class="btn" type="button" @click="transfer.refreshPeersAction()">
            <Icon name="refresh" :size="14" />{{ $t("main.refresh") }}
          </button>
        </div>
      </header>

      <div class="page-body">
        <section class="pick" id="pickCard">
          <header class="pick-head">
            <span class="pick-title">{{ $t("main.pickTitle") }}</span>
            <button
              id="pickClear"
              class="pick-x"
              type="button"
              :title="$t('main.clearPick')"
              :aria-label="$t('main.clearPick')"
              v-show="pickCount > 0"
              @click="transfer.clearPick()"
            >
              <Icon name="close" :size="14" />
            </button>
          </header>
          <div class="pick-meta">
            <span id="pickCount">{{ $t("main.pickCount", { n: pickCount }) }}</span>
            <span id="pickSize">{{ $t("main.pickSize", { size: transfer.fmtBytes(pickSize) }) }}</span>
          </div>
          <div class="pick-grid" id="pickGrid">
            <div v-for="(path, i) in transfer.pickPaths.value" :key="path" class="pick-item">
              <div class="pick-tile"><Icon name="file" :size="40" /></div>
              <div class="pick-name" :title="path" @click="$event.currentTarget.parentElement.classList.toggle('open')">
                {{ baseName(path) }}
              </div>
              <button
                class="pick-del"
                type="button"
                :title="$t('main.removePick')"
                :aria-label="$t('main.removePick')"
                @click.stop="transfer.removePick(i)"
              >
                <Icon name="deleteRound" :size="28" />
              </button>
            </div>
          </div>
          <div class="pick-hint" id="pickHint" v-show="pickCount === 0">
            {{ $t("main.pickHint") }}
          </div>
          <footer class="pick-foot">
            <button id="pickAdd" class="btn pick-add" type="button" @click="transfer.addFiles()">
              <Icon name="plus" :size="14" />{{ $t("main.pickAdd") }}
            </button>
          </footer>

          <div id="pickDrop" class="pick-drop" v-show="transfer.dragActive.value">
            <span class="pick-drop-icon"><Icon name="bigDownload" :size="96" /></span>
            <div class="pick-drop-text">{{ $t("main.pickDrop") }}</div>
          </div>
        </section>

        <div class="section-title">{{ $t("main.nearby") }}</div>
        <div id="peerList" class="device-list">
          <div
            v-for="peer in transfer.peers.value"
            :key="peer.fingerprint || peer.ip + ':' + peer.port"
            class="device-item"
            tabindex="0"
            role="button"
            :title="$t('main.sendTo', { name: peer.alias || peer.ip })"
            @click="transfer.sendToPeer(peer)"
            @keydown.enter.prevent="transfer.sendToPeer(peer)"
            @keydown.space.prevent="transfer.sendToPeer(peer)"
          >
            <span class="device-avatar"><Icon :name="transfer.deviceIcon(peer.device_type)" :size="20" /></span>
            <div class="device-meta">
              <div class="device-name">{{ peer.alias || $t("main.unknownDevice") }}</div>
              <div class="device-sub">
                {{ [peer.ip + ':' + peer.port, peer.device_model, peer.protocol].filter(Boolean).join(' · ') }}
              </div>
            </div>
            <span class="device-go"><Icon name="upload" :size="16" /></span>
          </div>
        </div>
        <div id="peerEmpty" class="empty" v-show="!transfer.peers.value.length">
          <span class="empty-ico"><Icon name="transfer" :size="30" /></span>
          <span>{{ $t("main.peerEmpty") }}</span>
        </div>
      </div>
    </div>

    <!-- 接收 -->
    <div v-show="sub === 'receive'" id="paneReceive" class="pane">
      <header class="head">
        <h1>{{ $t("main.receiveTitle") }}</h1>
        <div class="tools">
          <button id="tfRefreshRecv" class="btn" type="button" @click="transfer.refreshPending(); transfer.refreshLocalInfo()">
            <Icon name="refresh" :size="14" />{{ $t("main.refresh") }}
          </button>
        </div>
      </header>

      <div class="page-body rv-body">
        <div class="rv-hero">
          <div class="rv-logo" role="img" aria-label="Vexto"></div>
          <div class="rv-name" id="localAlias">{{ transfer.alias.value }}</div>
          <div class="rv-addr" id="localAddr">{{ transfer.addr.value }}</div>
        </div>

        <div class="rv-auto">
          <span class="rv-auto-label">{{ $t("main.autoSave") }}</span>
          <button
            id="autoSave"
            class="switch"
            type="button"
            role="switch"
            :aria-checked="transfer.autoSave.value ? 'true' : 'false'"
            :title="$t('main.autoSaveHint')"
            @click="transfer.toggleAutoSave()"
          >
            <span class="switch-knob"></span>
          </button>
        </div>

        <div class="rv-dir">
          <span class="path-text" id="localDir">{{ transfer.receiveDir.value || "—" }}</span>
          <button
            id="tfOpenDir"
            class="icon-btn"
            type="button"
            :title="$t('main.openReceiveDir')"
            :aria-label="$t('main.openReceiveDir')"
            @click="openDir"
          >
            <Icon name="folder" :size="16" />
          </button>
        </div>
      </div>
    </div>

    <!-- 历史 -->
    <div v-show="sub === 'history'" id="paneHistory" class="pane">
      <header class="head">
        <h1>{{ $t("main.historyTitle") }}</h1>
        <div class="tools">
          <span class="hs-per-page">
            <span class="hs-per-page-label">{{ $t("main.hsPerPageShort") }}</span>
            <SegmentedControl
              id="hsPageSize"
              class="hs-per-page-seg"
              :aria-label="$t('main.hsPageSize')"
              :model-value="String(transfer.hs.pageSize)"
              :options="PAGE_SIZES"
              @update:model-value="setPageSize"
            />
          </span>
          <button
            id="hsClear"
            class="btn"
            :class="{ armed: transfer.hs.clearArmed }"
            type="button"
            @click="transfer.onClearHistory($event.currentTarget)"
          >
            {{ transfer.hs.clearArmed ? $t("main.hsConfirmClear") : $t("main.hsClear") }}
          </button>
        </div>
      </header>

      <div class="page-body">
        <div id="hsList" class="hs-list">
          <div v-for="item in transfer.hs.items" :key="item.id" class="hs-row">
            <span class="hs-dir" :title="item.direction === 'send' ? $t('main.hsSend') : $t('main.hsReceive')">
              <Icon :name="item.direction === 'send' ? 'upload' : 'download'" :size="16" />
            </span>
            <div class="hs-meta">
              <div class="hs-name">{{ firstFileName(item) }}</div>
              <div class="hs-sub">{{ subText(item) }}</div>
            </div>
            <span
              class="hs-badge"
              :class="{ done: item.status === 'done', failed: item.status === 'failed' }"
            >
              {{ transfer.hsStatusText(item.status) }}
            </span>
            <button
              class="hs-more"
              type="button"
              aria-haspopup="menu"
              :aria-expanded="transfer.hs.menuOpen && transfer.hs.menuItem && transfer.hs.menuItem.id === item.id ? 'true' : 'false'"
              @click.stop="transfer.openHistoryMenu(item, $event.currentTarget)"
            >
              <Icon name="more" :size="16" />
            </button>
          </div>
        </div>
        <div id="hsEmpty" class="empty" v-show="!transfer.hs.items.length">
          <span class="empty-ico"><Icon name="history" :size="30" /></span>
          <span v-html="$t('main.hsEmpty')"></span>
        </div>
        <div id="hsPager" class="hs-pager" v-show="transfer.hs.total > 0">
          <button id="hsPrev" class="btn" type="button" :disabled="transfer.hs.page <= 1" @click="transfer.hsLoad(transfer.hs.page - 1)">
            {{ $t("main.hsPrev") }}
          </button>
          <span id="hsPageInfo" class="num">
            {{ $t("main.hsPageInfo", { page: transfer.hs.page, pages: pages(), total: transfer.hs.total }) }}
          </span>
          <button id="hsNext" class="btn" type="button" :disabled="transfer.hs.page >= pages()" @click="transfer.hsLoad(transfer.hs.page + 1)">
            {{ $t("main.hsNext") }}
          </button>
        </div>
      </div>

      <div
        id="hsMenu"
        class="hs-menu"
        role="menu"
        v-show="transfer.hs.menuOpen"
        :style="{ left: transfer.hs.menuLeft + 'px', top: transfer.hs.menuTop + 'px' }"
      >
        <button
          v-for="entry in menuEntries(transfer.hs.menuItem || {})"
          :key="entry.key"
          type="button"
          :class="{ danger: entry.danger }"
          :disabled="entry.disabled"
          @click="onMenuEntry(transfer.hs.menuItem, entry)"
        >
          <Icon :name="entry.icon" :size="16" /><span>{{ entry.label }}</span>
        </button>
      </div>

      <div id="hsInfo" class="tp-modal" v-show="transfer.hs.infoItem">
        <div class="tp-card" role="dialog" aria-modal="true">
          <div class="tp-card-title">{{ $t("main.hsInfoTitle") }}</div>
          <div class="hs-kv">
            <div v-for="([k, v], i) in infoRows(transfer.hs.infoItem || {})" :key="i" class="hs-kv-row">
              <div class="k">{{ k }}</div>
              <div class="v">{{ v }}</div>
            </div>
          </div>
          <div class="tp-card-acts">
            <button class="btn btn-primary" type="button" @click="transfer.closeHistoryInfo()">
              {{ $t("common.close") }}
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
