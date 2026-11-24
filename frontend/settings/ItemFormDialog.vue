<script setup>
import { onMounted, reactive, ref } from "vue";
import { t } from "../i18n/index.js";
import { itemShowIf } from "./utils.js";

const props = defineProps({
  title: { type: String, default: "" },
  fields: { type: Array, default: () => [] },
  testAction: { type: String, default: "" },
});
const emit = defineEmits(["save", "cancel"]);

const draft = reactive({});
// 弹窗这一轮的测试结论；改了任何字段就作废，免得拿旧配置的「通过」
// 去标一套已经改过的配置。
const dialogStatus = ref("idle");
const testing = ref(false);
const body = ref(null);

for (const fld of props.fields) {
  // select 的草稿值必须等于界面上默认选中的那一项：浏览器会自动选中第一个
  // option，但草稿若留空，项内 show_if 就认为「没选厂商」，两家的凭据行
  // 全被藏起来，连 APPID 都没地方填（会话 31 实机发现的坑）。
  draft[fld.key] =
    fld.type === "switch"
      ? false
      : fld.type === "select"
        ? String((fld.options && fld.options[0] && fld.options[0][0]) || "")
        : "";
}

const shown = (fld) => itemShowIf(fld, draft);

function setValue(key, value) {
  draft[key] = value;
  dialogStatus.value = "idle";
}

async function test() {
  if (testing.value || !props.testAction) return;
  testing.value = true;
  try {
    const res = await window.__TAURI__.core.invoke(props.testAction, {
      profile: JSON.parse(JSON.stringify(draft)),
    });
    dialogStatus.value = res && res.ok !== false ? "ok" : "bad";
  } catch {
    dialogStatus.value = "bad";
  } finally {
    testing.value = false;
  }
}

function save() {
  // 名称留空给个占位，别让卡片出现空标题。认 name / vendor / model
  // 三个通用键（两张列表都是这个形状），不是某个工具专属分支。
  if (!String(draft.name || "").trim()) {
    const vf = props.fields.find((x) => x.key === "vendor");
    const hit =
      vf &&
      (vf.options || []).find((o) => String(o[0]) === String(draft.vendor || ""));
    draft.name =
      (hit && String(hit[1])) ||
      String(draft.model || "").trim() ||
      t("settings.listUnnamed");
  }
  emit("save", JSON.parse(JSON.stringify(draft)), dialogStatus.value);
}

function focusFirst() {
  const node = body.value?.querySelector("input[type=text], select");
  if (node) node.focus();
}

onMounted(focusFirst);
</script>

<template>
  <div class="modal">
    <div class="modal-card modal-wide">
      <h2>{{ title }}</h2>
      <div ref="body" class="fm-body">
        <div
          v-for="fld in fields.filter(shown)"
          :key="fld.key"
          class="fm-row"
          :class="{ 'fm-switch': fld.type === 'switch' }"
        >
          <span class="fm-label">{{ fld.label || fld.key }}</span>
          <label v-if="fld.type === 'switch'" class="switch-pill sm">
            <input
              type="checkbox"
              :checked="!!draft[fld.key]"
              @change="setValue(fld.key, $event.target.checked)"
            />
            <span class="track"></span>
            <span class="thumb"></span>
          </label>
          <select
            v-else-if="fld.type === 'select'"
            :value="String(draft[fld.key] ?? '')"
            @change="setValue(fld.key, $event.target.value)"
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
            :value="draft[fld.key] ?? ''"
            :placeholder="fld.placeholder || ''"
            @input="setValue(fld.key, $event.target.value)"
            @keydown.enter.prevent="$event.target.blur()"
          />
        </div>
        <div
          v-if="dialogStatus !== 'idle'"
          class="result"
          :class="dialogStatus === 'ok' ? 'ok' : 'bad'"
        >
          {{ dialogStatus === "ok" ? $t("settings.listTestPass") : $t("settings.listTestFail") }}
        </div>
      </div>
      <div class="modal-acts">
        <button v-if="testAction" class="btn" type="button" :disabled="testing" @click="test">
          {{ testing ? $t("settings.listTesting") : $t("settings.listTest") }}
        </button>
        <button class="btn" type="button" @click="emit('cancel')">
          {{ $t("common.cancel") }}
        </button>
        <button class="btn btn-primary" type="button" @click="save">
          {{ $t("common.save") }}
        </button>
      </div>
    </div>
  </div>
</template>
