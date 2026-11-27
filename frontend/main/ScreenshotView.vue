<script setup>
import { computed, onBeforeUnmount, onMounted, reactive, ref } from "vue";
import Icon from "../components/Icon.vue";
import { useToast } from "../composables/useToast.js";
import { t } from "../i18n/index.js";

const toast = useToast();
const invoke = window.__TAURI__.core.invoke;

const items = ref([]);
const loading = ref(false);
const armed = reactive({});
const armTimers = new Map();
let loadSeq = 0;

const count = computed(() => items.value.length);

function errText(e) {
  return String(e && e.message ? e.message : e);
}

async function load() {
  const seq = ++loadSeq;
  loading.value = true;
  try {
    const list = await invoke("screenshot_gallery_list");
    // 删除后的自动刷新和手动刷新可能并发；只认最后一次请求的结果。
    if (seq === loadSeq) items.value = list;
  } catch (e) {
    if (seq === loadSeq) toast.show(errText(e), { kind: "error" });
  } finally {
    if (seq === loadSeq) loading.value = false;
  }
}

async function openDir() {
  try {
    const dir = await invoke("preview_save_dir");
    await invoke("open_path", { path: dir });
  } catch (e) {
    toast.show(errText(e), { kind: "error" });
  }
}

function pad(n) {
  return String(n).padStart(2, "0");
}

function fmtWhen(ms) {
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return "—";
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function fmtSize(n) {
  if (!n) return "0 B";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

async function preview(item) {
  try {
    const r = await invoke("screenshot_gallery_preview", { path: item.path });
    if (!r || !r.ok) toast.show((r && r.message) || "", { kind: "error" });
  } catch (e) {
    toast.show(errText(e), { kind: "error" });
  }
}

async function pin(item) {
  try {
    const r = await invoke("screenshot_gallery_pin", { path: item.path });
    if (r && r.ok) toast.show(t("main.shPinned"), { kind: "success" });
    else toast.show((r && r.message) || "", { kind: "error" });
  } catch (e) {
    toast.show(errText(e), { kind: "error" });
  }
}

async function copy(item) {
  try {
    const r = await invoke("screenshot_gallery_copy", { path: item.path });
    if (r && r.ok) toast.show(t("main.shCopied"), { kind: "success" });
    else toast.show((r && r.message) || "", { kind: "error" });
  } catch (e) {
    toast.show(errText(e), { kind: "error" });
  }
}

function remove(item) {
  if (!armed[item.path]) {
    armed[item.path] = true;
    armTimers.set(
      item.path,
      setTimeout(() => {
        delete armed[item.path];
        armTimers.delete(item.path);
      }, 3000),
    );
    return;
  }
  clearTimeout(armTimers.get(item.path));
  armTimers.delete(item.path);
  delete armed[item.path];
  invoke("screenshot_gallery_remove", { path: item.path })
    .then((r) => {
      if (r && r.ok) items.value = items.value.filter((x) => x.path !== item.path);
      else toast.show((r && r.message) || "", { kind: "error" });
    })
    .catch((e) => toast.show(errText(e), { kind: "error" }));
}

onMounted(load);

onBeforeUnmount(() => {
  for (const timer of armTimers.values()) clearTimeout(timer);
});
</script>

<template>
  <div id="screenshotView" class="main">
    <header class="head">
      <h1>{{ $t("main.navScreenshot") }}</h1>
      <div class="tools">
        <span class="sg-total">{{ $t("main.shCount", { n: count }) }}</span>
        <button class="btn" type="button" @click="load">
          <Icon name="refresh" :size="14" />{{ $t("main.shRefresh") }}
        </button>
        <button class="btn" type="button" @click="openDir">
          <Icon name="folder" :size="14" />{{ $t("main.shOpenDir") }}
        </button>
      </div>
    </header>

    <div class="page-body">
      <div v-if="!loading && !count" id="shGallery" class="empty">
        <span class="empty-ico"><Icon name="camera" :size="30" /></span>
        <span>{{ $t("main.shEmptyGallery") }}</span>
      </div>

      <div v-else id="shGallery" class="sg-grid">
        <article
          v-for="item in items"
          :key="item.path"
          class="sg-tile"
          tabindex="0"
          :title="$t('main.shPreviewHint')"
          @dblclick="preview(item)"
          @keydown.enter.prevent="preview(item)"
        >
          <img
            v-if="item.thumb"
            class="sg-img"
            :src="item.thumb"
            :alt="item.name"
            loading="lazy"
          />
          <span v-else class="sg-ph"><Icon name="camera" :size="28" /></span>

          <div class="sg-acts">
            <button class="sg-act" type="button" :title="$t('main.shPin')" @click.stop="pin(item)">
              <Icon name="pin" :size="14" />
            </button>
            <button class="sg-act" type="button" :title="$t('main.shCopy')" @click.stop="copy(item)">
              <Icon name="copy" :size="14" />
            </button>
            <button
              class="sg-act danger"
              :class="{ armed: armed[item.path] }"
              type="button"
              :title="armed[item.path] ? $t('main.confirmRemove') : $t('main.shDelete')"
              @click.stop="remove(item)"
            >
              <Icon name="trash" :size="14" />
            </button>
          </div>

          <div class="sg-meta">
            <span class="sg-name">{{ item.name }}</span>
            <span class="sg-sub">{{ fmtSize(item.size) }} · {{ fmtWhen(item.mtime_ms) }}</span>
          </div>
        </article>
      </div>
    </div>
  </div>
</template>
