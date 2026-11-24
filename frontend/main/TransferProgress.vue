<script setup>
import { computed, inject } from "vue";
import Icon from "../components/Icon.vue";
import { t } from "../i18n/index.js";

const transfer = inject("transfer");

const s = computed(() => transfer.tpCurrent());
const st = computed(() => transfer.tpStatusOf(s.value));
const sending = computed(() => !!s.value && s.value.direction === "send");
const finished = computed(() => ["done", "cancelled", "failed"].includes(st.value));

const title = computed(() => {
  if (st.value === "preparing") return t("main.tpPreparing");
  if (st.value === "done") return t("main.tpDone");
  if (st.value === "cancelled") return t("main.tpCancelled");
  if (st.value === "failed") return t("main.tpFailed");
  return sending.value ? t("main.tpSending") : t("main.tpReceiving");
});

const sub = computed(() => {
  const cur = s.value;
  if (!cur) return "";
  const peer = cur.peer || t("main.peerFallback");
  return sending.value
    ? t("main.tpTo", { peer })
    : t("main.tpFrom", {
        peer,
        dir: transfer.receiveDir.value
          ? t("main.tpSaveTo", { dir: transfer.receiveDir.value })
          : "",
      });
});

const files = computed(() => {
  const cur = s.value;
  if (!cur) return [];
  return cur.files && cur.files.length
    ? cur.files
    : [
        {
          name: cur.message || t("main.hsSession"),
          bytes: cur.bytes,
          total: cur.total,
          status: st.value,
        },
      ];
});

const totalPct = computed(() => transfer.tpPct(s.value));

const statText = computed(() => {
  const cur = s.value;
  const p = totalPct.value;
  if (!cur) return "";
  if (st.value === "preparing") return t("main.tpPreparingShort");
  if (st.value === "done") return t("main.tpDoneShort");
  if (st.value === "cancelled") return t("main.tpCancelled");
  if (st.value === "failed") {
    return cur.message
      ? t("main.tpFailedWith", { message: cur.message })
      : t("main.statusFailed");
  }
  let text = cur.total
    ? t("main.tpStatTotal", {
        got: transfer.fmtBytes(cur.bytes),
        total: transfer.fmtBytes(cur.total),
        pct: p,
      })
    : p + "%";
  if (cur.speed_bps) text += t("main.tpStatSpeed", { speed: transfer.fmtSpeed(cur.speed_bps) });
  return text;
});

const actionLabel = computed(() => {
  if (!finished.value) return t("main.tpCancel");
  return st.value === "done" ? t("main.tpFinished") : t("main.tpClose");
});

function fileRowClass(f) {
  return {
    done: f.status === "done",
    failed: f.status === "failed" && st.value !== "cancelled",
  };
}

function fileStatus(f) {
  if (st.value === "cancelled" && f.status !== "done") {
    return sending.value ? t("main.tpCancelled") : t("main.tpPeerCancelled");
  }
  if (f.status === "failed") return t("main.statusFailed");
  return transfer.tpFilePct(f) + "%";
}
</script>

<template>
  <section id="tp" class="tp" :class="{ hidden: !transfer.tpOpen.value }" :aria-label="$t('main.tpAriaLabel')">
    <header class="tp-head">
      <div class="tp-title">{{ title }}</div>
      <div class="tp-sub">{{ sub }}</div>
    </header>

    <div class="tp-list" id="tpFiles">
      <div v-for="(f, i) in files" :key="f.name + i" class="tp-file" :class="fileRowClass(f)">
        <span class="tp-file-ico"><Icon name="file" :size="20" /></span>
        <div class="tp-file-meta">
          <div class="tp-file-name">{{ f.name || $t("main.hsSession") }}</div>
          <div class="tp-file-sub">
            {{ f.status === 'done' ? transfer.fmtBytes(f.total || f.bytes) : transfer.fmtBytes(f.bytes) + ' / ' + transfer.fmtBytes(f.total) }}
          </div>
          <div class="tp-file-bar"><i :style="{ width: transfer.tpFilePct(f) + '%' }"></i></div>
        </div>
        <span class="tp-file-pct num">{{ fileStatus(f) }}</span>
      </div>
    </div>

    <footer class="tp-foot">
      <div class="tp-total">
        <div class="tp-total-row">
          <span class="tp-total-label">{{ $t("main.tpTotal") }}</span>
          <span class="num tp-total-stat">{{ statText }}</span>
        </div>
        <div class="tp-bar"><i :style="{ width: totalPct + '%' }"></i></div>
      </div>
      <button
        id="tpAction"
        class="btn"
        :class="{ 'btn-primary': !finished || st === 'done' }"
        type="button"
        @click="transfer.tpAction()"
      >
        {{ actionLabel }}
      </button>
    </footer>

    <div class="tp-modal" v-show="transfer.tpPendingCancel.value">
      <div class="tp-card" role="dialog" aria-modal="true">
        <div class="tp-card-title">{{ $t("main.tpConfirmTitle") }}</div>
        <div class="tp-card-text">{{ $t("main.tpConfirmText") }}</div>
        <div class="tp-card-acts">
          <button class="btn" type="button" @click="transfer.tpPendingClear()">
            {{ $t("main.tpContinue") }}
          </button>
          <button class="btn tp-danger" type="button" @click="transfer.tpConfirmCancel()">
            {{ $t("main.tpCancelTransfer") }}
          </button>
        </div>
      </div>
    </div>
  </section>
</template>
