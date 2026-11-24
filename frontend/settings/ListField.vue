<script setup>
import { computed, reactive, ref } from "vue";
import Icon from "../components/Icon.vue";
import ItemFormDialog from "./ItemFormDialog.vue";
import { t } from "../i18n/index.js";
import { stableJson, itemShowIf, errText, summaryParts } from "./utils.js";

const props = defineProps({
  field: { type: Object, required: true },
  settings: { type: Object, required: true },
});

const openIds = reactive(new Set());
const testing = reactive(new Set());
const statuses = reactive({});
const dialogOpen = ref(false);

const itemFields = computed(() => props.field.item_fields || []);
const items = computed(() => {
  const v = props.settings.val(props.field.key);
  return Array.isArray(v) ? v : [];
});

const statusText = computed(() => ({
  ok: t("settings.listTestPass"),
  bad: t("settings.listTestFail"),
  idle: t("settings.listIdle"),
}));

function statusKey(id) {
  return `${props.field.key}:${id}`;
}

function statusOf(item) {
  const rec = statuses[statusKey(item.id)];
  return rec && rec.sig === stableJson(item) ? rec.status : "idle";
}

function setStatus(item, status) {
  statuses[statusKey(item.id)] = { status, sig: stableJson(item) };
}

function partsOf(item) {
  return summaryParts(props.field, item);
}

function detailFields(item) {
  return itemFields.value.filter((fld) => itemShowIf(fld, item));
}

function patch(id, key, value) {
  const next = JSON.parse(JSON.stringify(items.value));
  const item = next.find((x) => x.id === id);
  if (!item) return;
  item[key] = value;
  props.settings.commit(props.field.key, next);
  // 配置变了，上一轮的测试结论不再代表当前值。
  delete statuses[statusKey(id)];
}

function toggleOpen(id) {
  if (openIds.has(id)) openIds.delete(id);
  else openIds.add(id);
}

async function testItem(item) {
  if (testing.has(item.id) || !props.field.test_action) return;
  testing.add(item.id);
  const fresh = JSON.parse(JSON.stringify(item));
  let res;
  try {
    res = await window.__TAURI__.core.invoke(props.field.test_action, {
      profile: fresh,
    });
  } catch (e) {
    res = { ok: false, message: errText(e) };
  } finally {
    testing.delete(item.id);
  }
  setStatus(fresh, res && res.ok !== false ? "ok" : "bad");
}

function removeItem(item) {
  const next = items.value.filter((x) => x.id !== item.id);
  props.settings.commit(props.field.key, next);
  // 删的正好是「当前使用」→ 顺位：厂商优先、OpenAI 其次。
  const prefix = props.field.key === "vendor_profiles" ? "vendor:" : "openai:";
  const ae = String(props.settings.val("active_engine") || "");
  if (ae === prefix + item.id || ae === prefix + item.id + ":llm") {
    const vendors =
      props.field.key === "vendor_profiles"
        ? next
        : props.settings.val("vendor_profiles") || [];
    const models =
      props.field.key === "openai_profiles"
        ? next
        : props.settings.val("openai_profiles") || [];
    const pick = vendors[0]
      ? "vendor:" + vendors[0].id
      : models[0]
        ? "openai:" + models[0].id
        : "microsoft";
    props.settings.commit("active_engine", pick);
  }
}

function addTitle() {
  const noun = String(props.field.label || "一条").replace(/^已配置的/, "");
  return t("settings.formAdd", { noun: noun || "一条" });
}

function openAdd() {
  dialogOpen.value = true;
}

function onSave(draft, status) {
  const item = Object.assign(
    { id: "m" + Date.now().toString(36) + Math.random().toString(36).slice(2, 8) },
    draft,
  );
  if (status && status !== "idle") setStatus(item, status);
  const next = JSON.parse(JSON.stringify(items.value));
  next.push(item);
  props.settings.commit(props.field.key, next);
  // 第一个添加的直接设为当前使用，否则用户以为没生效
  if (!String(props.settings.val("active_engine") || "")) {
    const prefix = props.field.key === "vendor_profiles" ? "vendor:" : "openai:";
    props.settings.commit("active_engine", prefix + item.id);
  }
  dialogOpen.value = false;
}
</script>

<template>
  <div class="field list">
    <div class="subhead">{{ field.label }}</div>
    <div v-if="field.help" class="subhead-desc">{{ field.help }}</div>

    <div class="list-body">
      <div v-if="!items.length" class="list-empty">{{ $t("settings.listEmpty") }}</div>

      <div v-for="item in items" :key="item.id" class="list-item" :class="{ open: openIds.has(item.id) }">
        <div class="li-head">
          <button
            type="button"
            class="li-arrow"
            :title="$t('settings.listExpand')"
            :aria-label="$t('settings.listExpand')"
            @click="toggleOpen(item.id)"
          ></button>
          <span class="li-ico"><Icon name="file" :size="16" /></span>

          <div class="li-info">
            <div class="li-name-row">
              <span class="li-name">{{ partsOf(item)[0] || $t("settings.listUnnamed") }}</span>
              <span
                v-if="field.test_action"
                class="li-status"
                :data-status="statusOf(item)"
                :title="statusText[statusOf(item)]"
              ></span>
            </div>
            <span v-if="partsOf(item).length > 1" class="li-desc">
              {{ partsOf(item).slice(1).join(" · ") }}
            </span>
          </div>

          <button
            v-if="field.test_action"
            type="button"
            class="btn li-test"
            :disabled="testing.has(item.id)"
            :title="$t('settings.listTestTitle')"
            @click="testItem(item)"
          >
            {{ testing.has(item.id) ? $t("settings.listTesting") : $t("settings.listTest") }}
          </button>

          <button
            type="button"
            class="btn li-del"
            :title="$t('settings.listDeleteTitle')"
            @click="removeItem(item)"
          >
            {{ $t("settings.listDelete") }}
          </button>

          <label class="switch-pill sm li-enable" :title="$t('settings.listEnableTitle')">
            <input
              type="checkbox"
              :checked="item.enabled !== false"
              @change="patch(item.id, 'enabled', $event.target.checked)"
            />
            <span class="track"></span>
            <span class="thumb"></span>
          </label>
        </div>

        <div v-if="openIds.has(item.id)" class="li-detail">
          <div v-for="fld in detailFields(item)" :key="fld.key" class="li-row">
            <span class="li-label">{{ fld.label || fld.key }}</span>
            <label v-if="fld.type === 'switch'" class="switch-pill sm">
              <input
                type="checkbox"
                :checked="!!item[fld.key]"
                @change="patch(item.id, fld.key, $event.target.checked)"
              />
              <span class="track"></span>
              <span class="thumb"></span>
            </label>
            <select
              v-else-if="fld.type === 'select'"
              :value="String(item[fld.key] ?? '')"
              @change="patch(item.id, fld.key, $event.target.value)"
            >
              <option v-for="[value, label] in fld.options || []" :key="value" :value="value">
                {{ label }}
              </option>
            </select>
            <input
              v-else
              class="input"
              :type="fld.type === 'password' ? 'password' : 'text'"
              spellcheck="false"
              :value="item[fld.key] ?? ''"
              :placeholder="fld.placeholder || ''"
              @input="patch(item.id, fld.key, $event.target.value)"
            />
          </div>
        </div>
      </div>

      <button type="button" class="btn list-add" @click="openAdd">
        <Icon name="plus" :size="15" />
        <span>{{ $t("settings.listAdd") }}</span>
      </button>
    </div>

    <ItemFormDialog
      v-if="dialogOpen"
      :title="addTitle()"
      :fields="itemFields"
      :test-action="field.test_action || ''"
      @save="onSave"
      @cancel="dialogOpen = false"
    />
  </div>
</template>
