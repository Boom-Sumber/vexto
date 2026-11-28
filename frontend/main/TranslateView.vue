<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import Icon from "../components/Icon.vue";
import Tabs from "../components/Tabs.vue";
import DropdownSelect from "../components/DropdownSelect.vue";
import { useToast } from "../composables/useToast.js";
import { t } from "../i18n/index.js";

const toast = useToast();
const invoke = window.__TAURI__.core.invoke;

const sub = ref("history");

const source = ref("");
const result = ref("");
const direction = ref("");
const busy = ref(false);
const sourceEl = ref(null);

const entries = ref([]);
const engineFilter = ref("");
const rangeFilter = ref("all");
const keyword = ref("");
const page = ref(1);
const PAGE_SIZE = 20;
const armed = reactive({});
const armTimers = new Map();
const clearArmed = ref(false);
let clearTimer = null;

const items = computed(() => [
  { id: "translate", label: t("main.trTabTranslate") },
  { id: "history", label: t("main.trTabHistory") },
]);

const engineOptions = computed(() => {
  const seen = new Map();
  for (const e of entries.value) {
    if (e.engine) seen.set(e.engine, e.engine_name || e.engine);
  }
  return [["", t("main.trAllEngines")], ...[...seen].map(([id, name]) => [id, name])];
});

const rangeOptions = computed(() => [
  ["all", t("main.trRangeAll")],
  ["today", t("main.trToday")],
  ["7d", t("main.tr7d")],
  ["30d", t("main.tr30d")],
]);

function startOfToday() {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

const cutoff = computed(() => {
  if (rangeFilter.value === "today") return startOfToday();
  if (rangeFilter.value === "7d") return Date.now() - 7 * 86400000;
  if (rangeFilter.value === "30d") return Date.now() - 30 * 86400000;
  return 0;
});

const filtered = computed(() => {
  const kw = keyword.value.trim().toLowerCase();
  return entries.value.filter(
    (e) =>
      (!engineFilter.value || e.engine === engineFilter.value) &&
      e.at_ms >= cutoff.value &&
      (!kw || e.src.toLowerCase().includes(kw) || e.dst.toLowerCase().includes(kw)),
  );
});

const totalPages = computed(() => Math.max(1, Math.ceil(filtered.value.length / PAGE_SIZE)));
const pageItems = computed(() =>
  filtered.value.slice((page.value - 1) * PAGE_SIZE, page.value * PAGE_SIZE),
);

watch([engineFilter, rangeFilter, keyword], () => {
  page.value = 1;
});
watch(totalPages, (n) => {
  if (page.value > n) page.value = n;
});

function pad(n) {
  return String(n).padStart(2, "0");
}

function fmtWhen(ms) {
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return "—";
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function dirOf(e) {
  return `${(e.from || "").toUpperCase()} → ${(e.to || "").toUpperCase()}`;
}

async function load() {
  try {
    entries.value = await invoke("translate_history_list");
  } catch (e) {
    toast.show(String(e && e.message ? e.message : e), { kind: "error" });
  }
}

async function copyEntry(e) {
  try {
    await navigator.clipboard.writeText(e.dst);
    toast.show(t("main.trCopied"), { kind: "success" });
  } catch (err) {
    toast.show(String(err && err.message ? err.message : err), { kind: "error" });
  }
}

function onDelete(e) {
  if (!armed[e.id]) {
    armed[e.id] = true;
    armTimers.set(
      e.id,
      setTimeout(() => {
        delete armed[e.id];
        armTimers.delete(e.id);
      }, 3000),
    );
    return;
  }
  clearTimeout(armTimers.get(e.id));
  armTimers.delete(e.id);
  delete armed[e.id];
  invoke("translate_history_remove", { id: e.id })
    .then(load)
    .catch((err) => toast.show(String(err && err.message ? err.message : err), { kind: "error" }));
}

function onClear() {
  if (!entries.value.length) return;
  if (!clearArmed.value) {
    clearArmed.value = true;
    clearTimer = setTimeout(() => {
      clearArmed.value = false;
    }, 3000);
    return;
  }
  clearTimeout(clearTimer);
  clearArmed.value = false;
  invoke("translate_history_clear")
    .then(() => {
      entries.value = [];
      page.value = 1;
    })
    .catch((err) => toast.show(String(err && err.message ? err.message : err), { kind: "error" }));
}

async function runTranslate() {
  const text = source.value.trim();
  if (!text || busy.value) return;
  busy.value = true;
  try {
    const r = await invoke("translate_text_only", { text });
    result.value = r.dst || "";
    direction.value = `${(r.from || "").toUpperCase()} → ${(r.to || "").toUpperCase()}`;
    // 翻译已写入历史；刷新列表，切过去就能看到
    await load();
  } catch (e) {
    toast.show(String(e && e.message ? e.message : e), { kind: "error" });
  } finally {
    busy.value = false;
  }
}

function clearInput() {
  source.value = "";
  result.value = "";
  direction.value = "";
  sourceEl.value?.focus();
}

async function copyResult() {
  if (!result.value) return;
  try {
    await navigator.clipboard.writeText(result.value);
    toast.show(t("main.trCopied"), { kind: "success" });
  } catch (e) {
    toast.show(String(e && e.message ? e.message : e), { kind: "error" });
  }
}

watch(sub, (next) => {
  if (next === "translate") nextTick(() => sourceEl.value?.focus());
});

let unlistenHistory = null;

onMounted(async () => {
  await load();
  try {
    unlistenHistory = await window.__TAURI__.event.listen("translate:recorded", () => load());
  } catch {
    // 事件通道不可用时仍可手动切页刷新
  }
});

onBeforeUnmount(() => {
  if (typeof unlistenHistory === "function") unlistenHistory();
  for (const timer of armTimers.values()) clearTimeout(timer);
  if (clearTimer) clearTimeout(clearTimer);
});
</script>

<template>
  <div id="translateView" class="main">
    <div class="view-tabs">
      <Tabs v-model="sub" :items="items" />
    </div>

    <template v-if="sub === 'history'">
      <header class="head">
        <h1>{{ $t("main.navTranslate") }}</h1>
        <div class="tools">
          <span class="tr-total">{{ $t("main.trCount", { n: filtered.length }) }}</span>
          <button
            id="trClear"
            class="btn"
            :class="{ armed: clearArmed }"
            type="button"
            :disabled="!entries.length"
            @click="onClear"
          >
            {{ clearArmed ? $t("main.trConfirmClear") : $t("main.trClear") }}
          </button>
        </div>
      </header>

      <div class="tr-filters">
        <DropdownSelect v-model="engineFilter" :options="engineOptions" />
        <DropdownSelect v-model="rangeFilter" :options="rangeOptions" />
        <input
          id="trKeyword"
          v-model="keyword"
          class="tr-search"
          type="search"
          :placeholder="$t('main.trSearch')"
        />
      </div>

      <div class="page-body">
        <div v-if="!filtered.length" id="trEmpty" class="empty">
          <span class="empty-ico"><Icon name="history" :size="30" /></span>
          <span>{{ entries.length ? $t("main.trNoMatch") : $t("main.trEmptyHistory") }}</span>
        </div>

        <div v-else class="tr-list">
          <article v-for="e in pageItems" :key="e.id" class="tr-row">
            <div class="tr-src" :title="e.src">{{ e.src }}</div>
            <div class="tr-dst" :title="e.dst">{{ e.dst }}</div>
            <div class="tr-foot">
              <div class="tr-meta">
                <span class="tr-engine">{{ e.engine_name || e.engine }}</span>
                <span>{{ dirOf(e) }}</span>
                <span class="num">{{ fmtWhen(e.at_ms) }}</span>
              </div>
              <div class="tr-acts">
                <button class="btn" type="button" @click="copyEntry(e)">
                  {{ $t("main.trCopy") }}
                </button>
                <button
                  class="btn"
                  :class="{ armed: armed[e.id] }"
                  type="button"
                  @click="onDelete(e)"
                >
                  {{ armed[e.id] ? $t("main.confirmRemove") : $t("main.remove") }}
                </button>
              </div>
            </div>
          </article>
        </div>

        <div v-if="totalPages > 1" class="tr-pager">
          <button class="btn" type="button" :disabled="page <= 1" @click="page--">
            {{ $t("main.hsPrev") }}
          </button>
          <span class="num">{{ $t("main.trPage", { page, pages: totalPages }) }}</span>
          <button class="btn" type="button" :disabled="page >= totalPages" @click="page++">
            {{ $t("main.hsNext") }}
          </button>
        </div>
      </div>
    </template>

    <template v-else>
      <header class="head">
        <h1>{{ $t("main.navTranslate") }}</h1>
      </header>
      <div class="page-body trf-body">
        <section class="trf-card">
          <textarea
            id="trSource"
            ref="sourceEl"
            v-model="source"
            class="trf-input"
            :placeholder="$t('main.trPlaceholder')"
            spellcheck="false"
            @keydown.ctrl.enter.prevent="runTranslate"
          ></textarea>
          <div class="trf-bar">
            <span class="trf-count">{{ $t("main.trCharCount", { n: source.length }) }}</span>
            <div class="tools">
              <button class="btn" type="button" :disabled="!source" @click="clearInput">
                {{ $t("main.trClearInput") }}
              </button>
              <button
                id="trRun"
                class="btn btn-primary"
                type="button"
                :disabled="busy || !source.trim()"
                @click="runTranslate"
              >
                {{ busy ? $t("main.trTranslating") : $t("main.trAction") }}
              </button>
            </div>
          </div>
        </section>

        <section class="trf-card trf-result">
          <div class="trf-result-head">
            <span class="trf-dir">{{ direction }}</span>
            <button class="btn" type="button" :disabled="!result" @click="copyResult">
              {{ $t("main.trCopy") }}
            </button>
          </div>
          <div v-if="result" id="trResult" class="trf-output">{{ result }}</div>
          <div v-else class="trf-empty">{{ $t("main.trResultEmpty") }}</div>
        </section>
      </div>
    </template>
  </div>
</template>

<style scoped>
.page-body {
  padding-top: var(--sp-3);
}
</style>
