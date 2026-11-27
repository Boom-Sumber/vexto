<script setup>
import { computed, nextTick, onMounted, ref } from "vue";
import TitleBar from "../components/TitleBar.vue";
import Icon from "../components/Icon.vue";
import ToastHost from "../components/ToastHost.vue";
import FieldRenderer from "./FieldRenderer.vue";
import { useSettings } from "./useSettings.js";
import { useToast } from "../composables/useToast.js";
import { Icons } from "../shared/icons.js";

const s = useSettings();
const toast = useToast();
const {
  visibleTools,
  tool,
  sections,
  tabId,
  fields,
  pendingCount,
  status,
  statusKind,
  banner,
  loading,
  dirtyModal,
  dangerModal,
  modalText,
} = s;

const tb = ref(null);
const badge = s.update.badge;

function openToolbox() {
  window.__TAURI__.core
    .invoke("open_toolbox")
    .catch((e) => toast.show(String(e && e.message ? e.message : e), { kind: "error" }));
}

const blocks = computed(() => {
  const out = [];
  let card = null;
  for (const f of fields.value) {
    if (f.type === "group") {
      card = null;
      out.push({ type: "group", field: f });
      continue;
    }
    if (f.type === "list" || f.type === "about" || f.type === "tool_list") {
      card = null;
      out.push({ type: "single", field: f });
      continue;
    }
    if (f.type === "note" && !card) {
      out.push({ type: "single", field: f });
      continue;
    }
    if (!card) {
      card = { type: "card", fields: [] };
      out.push(card);
    }
    card.fields.push(f);
  }
  return out;
});

const statusLineKind = computed(() => {
  if (!tool.value || tool.value.enabled === false) return "off";
  if (tool.value.status_kind === "error") return "error";
  if (tool.value.status_kind === "warn") return "warn";
  return "";
});

async function onSave() {
  await s.saveAll();
}

onMounted(async () => {
  await s.init();
  await nextTick();
  tb.value?.reveal();
});
</script>

<template>
  <div class="win">
    <TitleBar ref="tb" :title="$t('settings.title')">
      <template #actions>
        <button
          type="button"
          class="tb-btn"
          :title="$t('common.openToolbox')"
          :aria-label="$t('common.openToolbox')"
          @click="openToolbox"
        >
          <Icon name="grid" :size="14" />
        </button>
      </template>
    </TitleBar>

    <div class="app">
      <aside class="sidebar">
        <div class="brand">
          <span class="logo logo-badge" aria-hidden="true"></span>
          <span class="brand-name">Vexto</span>
        </div>
        <nav id="tools" class="tools">
          <button
            v-for="t in visibleTools"
            :key="t.id"
            type="button"
            class="tool"
            :class="{ on: t.id === tool?.id }"
            @click="s.selectTool(t.id)"
          >
            <span v-if="t.icon && Icons.has(t.icon)" class="t-ico">
              <Icon :name="t.icon" :size="20" />
            </span>
            <span v-else class="dot"></span>
            <span class="t-name">{{ t.name }}</span>
          </button>
        </nav>
        <!-- 发现新版本时出现在导航栏最下面；点击跳到「关于 → 更新」。 -->
        <button
          v-if="badge.show"
          id="update-badge"
          class="update-badge"
          :class="{ required: badge.required }"
          type="button"
          @click="s.update.badgeClick()"
        >
          <span class="ub-dot" aria-hidden="true"></span>
          <span id="update-badge-text">{{ badge.text }}</span>
        </button>
      </aside>

      <main class="pane">
        <header id="pane-head" class="pane-head" v-show="tool && !tool.hide_header">
          <div class="pane-title">
            <h1 id="tool-name">{{ tool?.name || $t("settings.loading") }}</h1>
            <p id="tool-desc" class="hint">{{ tool?.desc || "" }}</p>
            <p id="tool-status-line" class="status-line" :class="statusLineKind">
              {{ tool?.status_line || "" }}
            </p>
          </div>
        </header>

        <div id="tool-status" class="tool-status" v-show="banner">{{ banner }}</div>

        <div id="tabs" class="tabs" v-show="sections.length > 1">
          <button
            v-for="sec in sections"
            :key="sec.id"
            type="button"
            class="tab"
            :class="{ on: sec.id === tabId }"
            @click="tabId = sec.id"
          >
            {{ sec.name }}
          </button>
        </div>

        <div class="pane-body">
          <div id="fields" class="fields">
            <template v-for="block in blocks" :key="block.type === 'card' ? block.fields[0]?.key : block.field.key">
              <div v-if="block.type === 'group'" class="group-title">{{ block.field.title }}</div>
              <div v-else-if="block.type === 'card'" class="card">
                <FieldRenderer
                  v-for="f in block.fields"
                  :key="f.key"
                  :field="f"
                  :settings="s"
                />
              </div>
              <FieldRenderer v-else :field="block.field" :settings="s" />
            </template>
          </div>
        </div>

        <footer class="pane-foot">
          <span id="status" :class="statusKind">{{ status }}</span>
          <div id="dirty-acts" class="dirty-acts" v-show="pendingCount > 0">
            <button id="btnDiscard" class="btn" type="button" @click="s.discardAll()">
              {{ $t("settings.discard") }}
            </button>
            <button id="btnSave" class="btn btn-primary" type="button" @click="onSave">
              {{ $t("settings.save") }}
            </button>
          </div>
        </footer>
      </main>
    </div>

    <div v-if="dirtyModal" id="dirty-modal" class="modal">
      <div class="modal-card">
        <h2>{{ $t("settings.dirtyTitle") }}</h2>
        <p class="modal-text">{{ modalText }}</p>
        <div class="modal-acts">
          <button id="mCancel" class="btn" type="button" @click="s.resolveDirty('cancel')">
            {{ $t("common.cancel") }}
          </button>
          <button id="mDiscard" class="btn" type="button" @click="s.resolveDirty('discard')">
            {{ $t("settings.discardChanges") }}
          </button>
          <button id="mSave" class="btn btn-primary" type="button" @click="s.resolveDirty('save')">
            {{ $t("settings.save") }}
          </button>
        </div>
      </div>
    </div>

    <div v-if="dangerModal" id="danger-modal" class="modal">
      <div class="modal-card">
        <h2>{{ $t("settings.resetAllTitle") }}</h2>
        <p class="modal-text">{{ $t("settings.resetAllConfirm") }}</p>
        <div class="modal-acts">
          <button id="dCancel" class="btn" type="button" @click="s.resolveDanger(false)">
            {{ $t("common.cancel") }}
          </button>
          <button id="dConfirm" class="btn danger" type="button" @click="s.resolveDanger(true)">
            {{ $t("settings.resetAllConfirmBtn") }}
          </button>
        </div>
      </div>
    </div>

    <ToastHost />
  </div>
</template>
