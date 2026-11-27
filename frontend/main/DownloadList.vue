<script setup>
import { computed, inject, onBeforeUnmount, reactive } from "vue";
import Icon from "../components/Icon.vue";
import Tabs from "../components/Tabs.vue";
import GithubView from "./GithubView.vue";
import { t } from "../i18n/index.js";
import { fmtBytes, fmtEta, fmtSpeed } from "./format.js";

const downloads = inject("downloads");
const {
  filter,
  error,
  global,
  tasks,
  counts,
  visibleTasks,
  uiFlags,
  titles,
  emptyIcon,
  emptyText,
} = downloads;

const CATS = [
  ["all", "main.catAll"],
  ["running", "main.catRunning"],
  ["failed", "main.catFailed"],
  ["complete", "main.catComplete"],
  ["deleted", "main.catDeleted"],
];

const catItems = computed(() => [
  ...CATS.map(([id, key]) => ({ id, label: t(key), badge: counts.value[id] || 0 })),
  { id: "github", label: t("main.catGithub") },
]);

const armed = reactive({});
const armTimers = new Map();

const STATUS = {
  active: "main.statusActive",
  waiting: "main.statusWaiting",
  paused: "main.statusPaused",
  complete: "main.statusDone",
  error: "main.statusError",
  removed: "main.statusRemoved",
};

function statusText(status) {
  return STATUS[status] ? t(STATUS[status]) : status || t("main.statusUnknown");
}

function pct(task) {
  if (!task.total_length) return 0;
  return Math.min(100, Math.round((task.completed_length / task.total_length) * 100));
}

function metaText(task) {
  const deleted = task.status === "complete" && task.file_missing;
  const parts = [];
  if (task.historical) parts.push(t("main.metaHistory"));
  parts.push(deleted ? t("main.metaFileDeleted") : statusText(task.status));
  if (task.status === "active") {
    parts.push(fmtSpeed(task.download_speed));
    parts.push(t("main.metaRemaining", { eta: fmtEta(task.eta_seconds, t) }));
  }
  parts.push(
    fmtBytes(task.completed_length) +
      " / " +
      (task.total_length ? fmtBytes(task.total_length) : "—"),
  );
  if (task.error_message) parts.push(task.error_message);
  return parts.join("  ·  ");
}

function toggleLabel(task) {
  if (["complete", "error", "removed"].includes(task.status)) return t("main.actFinished");
  return task.status === "paused" ? t("main.actResume") : t("main.actPause");
}

function toggleAction(task) {
  if (task.status === "paused") downloads.resume(task.gid);
  else downloads.pause(task.gid);
}

function removeTitle() {
  if (!uiFlags.value.delete_file) return "";
  return uiFlags.value.delete_mode === "permanent"
    ? t("main.removePermanentHint")
    : t("main.removeRecycleHint");
}

function disarm(gid) {
  const timer = armTimers.get(gid);
  if (timer) clearTimeout(timer);
  armTimers.delete(gid);
  delete armed[gid];
}

function onRemove(task) {
  if (!uiFlags.value.confirm) {
    downloads.remove(task.gid);
    return;
  }
  if (armed[task.gid]) {
    disarm(task.gid);
    downloads.remove(task.gid);
    return;
  }
  armed[task.gid] = true;
  armTimers.set(
    task.gid,
    setTimeout(() => disarm(task.gid), 3000),
  );
}

onBeforeUnmount(() => {
  for (const timer of armTimers.values()) clearTimeout(timer);
});
</script>

<template>
  <div id="downloadView" class="main">
    <div class="view-tabs" id="catsWrap">
      <Tabs :model-value="filter" :items="catItems" @update:model-value="downloads.setFilter" />
    </div>

    <header v-show="filter !== 'github'" class="head">
      <h1 id="title">{{ titles[filter] || titles.all }}</h1>
      <div class="tools">
        <button id="newTask" class="btn btn-primary" type="button" @click="downloads.newTask()">
          {{ $t("main.newTask") }}
        </button>
        <button id="clear" class="btn" type="button" @click="downloads.clearFinished()">
          {{ $t("main.clearFinished") }}
        </button>
      </div>
    </header>

    <div
      id="error"
      class="notice notice-danger error"
      :class="{ hidden: !error || filter === 'github' }"
    >
      {{ error }}
    </div>

    <main id="list" class="list" v-show="filter !== 'github'">
      <div
        v-for="task in visibleTasks"
        :key="task.gid"
        class="row"
        :class="{ deleted: task.status === 'complete' && task.file_missing }"
      >
        <div class="row-top">
          <span class="name" :title="task.file_path || task.name || ''">
            {{ task.name || $t("main.unknownFile") }}
          </span>
          <span class="pct">{{ task.total_length ? pct(task) + "%" : "—" }}</span>
        </div>
        <div class="bar"><i :style="{ width: pct(task) + '%' }"></i></div>
        <div class="meta" :class="{ bad: task.status === 'error' }">{{ metaText(task) }}</div>
        <div class="acts">
          <button
            type="button"
            class="btn"
            :disabled="['complete', 'error', 'removed'].includes(task.status)"
            @click="toggleAction(task)"
          >
            {{ toggleLabel(task) }}
          </button>
          <button
            type="button"
            class="btn"
            :class="{ armed: armed[task.gid] }"
            :title="removeTitle()"
            @click="onRemove(task)"
          >
            {{ armed[task.gid] ? $t("main.confirmRemove") : $t("main.remove") }}
          </button>
          <button type="button" class="btn" @click="downloads.openDir(task.gid)">
            {{ $t("main.openDir") }}
          </button>
        </div>
      </div>
    </main>

    <div v-show="filter !== 'github' && !visibleTasks.length" id="empty" class="empty">
      <span class="empty-ico"><Icon :name="emptyIcon[filter] || 'download'" :size="30" /></span>
      <span v-html="emptyText[filter] || emptyText.all"></span>
    </div>

    <footer v-show="filter !== 'github'" id="taskFoot" class="foot">
      <span id="counts" class="num">
        {{ $t("main.footCounts", { total: tasks.length, active: global.num_active || 0 }) }}
      </span>
    </footer>

    <main id="githubView" class="gh" v-show="filter === 'github'">
      <GithubView v-if="filter === 'github'" @show-all="downloads.setFilter('all')" />
    </main>
  </div>
</template>
