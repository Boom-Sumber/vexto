<script setup>
import { nextTick, onMounted, ref } from "vue";
import TitleBar from "../components/TitleBar.vue";
import Icon from "../components/Icon.vue";
import Button from "../components/Button.vue";
import TextField from "../components/TextField.vue";
import SelectField from "../components/SelectField.vue";
import Switch from "../components/Switch.vue";
import Tabs from "../components/Tabs.vue";
import ToastHost from "../components/ToastHost.vue";
import { useTheme } from "../composables/useTheme.js";
import { useToast } from "../composables/useToast.js";

const tb = ref(null);
const { mode, resolved } = useTheme();
const { show, loading } = useToast();

const text = ref("");
const num = ref(8);
const sel = ref("a");
const on = ref(true);
const tab = ref("one");

const options = [
  { value: "a", label: "选项 A" },
  { value: "b", label: "选项 B" },
  { value: "c", label: "选项 C" },
];
const tabs = [
  { id: "one", label: "第一页" },
  { id: "two", label: "第二页" },
  { id: "three", label: "第三页" },
];

function notify(kind) {
  if (kind === "loading") {
    const h = loading("正在处理…");
    setTimeout(() => h.update("处理完成", { kind: "success" }), 1200);
    return;
  }
  show(kind === "error" ? "保存失败：示例错误" : "保存成功", { kind });
}

onMounted(async () => {
  await nextTick();
  tb.value?.reveal();
});
</script>

<template>
  <div class="win">
    <TitleBar ref="tb" title="组件探针" />
    <div class="app probe">
      <div class="probe-head">
        <h1>公共层探针</h1>
        <span class="probe-meta">{{ mode }} · {{ resolved }}</span>
      </div>

      <section class="probe-card">
        <h2>按钮</h2>
        <div class="row">
          <Button variant="primary" @click="notify('success')">主要按钮</Button>
          <Button @click="notify('info')">次要按钮</Button>
          <Button variant="ghost" @click="notify('error')">错误提示</Button>
          <Button @click="notify('loading')">加载提示</Button>
          <Button @click="show('常驻提示', { kind: 'info', duration: 60000 })">常驻提示</Button>
        </div>
      </section>

      <section class="probe-card">
        <h2>表单</h2>
        <TextField v-model="text" label="文本" placeholder="输入一些文字" help="这是帮助文字" />
        <TextField v-model="num" label="数字" type="number" :min="0" :max="100" stepper />
        <SelectField v-model="sel" label="下拉" :options="options" />
        <Switch v-model="on" label="开关" />
        <Switch v-model="on" label="小号开关" size="sm" />
      </section>

      <section class="probe-card">
        <h2>标签页</h2>
        <Tabs v-model="tab" :items="tabs" />
        <p class="probe-text">当前页签：{{ tab }}</p>
      </section>

      <section class="probe-card">
        <h2>图标</h2>
        <div class="row">
          <Icon name="folder" :size="16" />
          <Icon name="download" :size="16" />
          <Icon name="info" :size="16" />
          <Icon name="alert" :size="16" />
          <Icon name="close" :size="14" />
        </div>
      </section>
    </div>
    <ToastHost />
  </div>
</template>

<style scoped>
.probe {
  padding: var(--sp-6);
  display: flex;
  flex-direction: column;
  gap: var(--sp-5);
  overflow: auto;
}
.probe-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: var(--sp-4);
}
.probe-head h1 {
  margin: 0;
  font-size: var(--fs-xl);
  font-weight: 700;
}
.probe-meta {
  color: var(--text-muted);
  font-size: var(--fs-sm);
}
.probe-card {
  display: flex;
  flex-direction: column;
  gap: var(--sp-3);
  padding: var(--sp-4);
  border: 1px solid var(--border-subtle);
  border-radius: var(--r-lg);
  background: var(--bg-panel);
}
.probe-card h2 {
  margin: 0;
  font-size: var(--fs-lg);
  font-weight: 600;
}
.row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--sp-3);
}
.probe-text {
  margin: 0;
  color: var(--text-secondary);
  font-size: var(--fs-sm);
}
</style>
