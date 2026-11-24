<script setup>
import { computed, onMounted, ref } from "vue";
import Icon from "../components/Icon.vue";
import { useToast } from "../composables/useToast.js";
import { useI18n } from "../i18n/index.js";
import { fmtBytes, fmtCount } from "./format.js";
import { useGithub } from "./useGithub.js";

const emit = defineEmits(["show-all"]);
const toast = useToast();
const { t } = useI18n();
const inputEl = ref(null);

const gh = useGithub({ toast, onShowAll: () => emit("show-all") });

const rateText = computed(() => {
  if (gh.loading.value) return t("main.ghConnecting");
  if (!gh.rate.value || gh.rate.value.remaining == null) return "";
  const limit = gh.rate.value.limit || 60;
  return (
    t("main.ghRate", { remaining: gh.rate.value.remaining, limit }) +
    (gh.rate.value.cached ? t("main.ghRateCached") : "")
  );
});

const rateWarn = computed(() => {
  const r = gh.rate.value;
  if (!r || r.remaining == null) return false;
  const limit = r.limit || 60;
  return r.remaining <= Math.min(5, limit);
});

const repoMeta = computed(() => {
  if (!gh.releases.value.length) return "";
  const total = gh.totalAssets();
  const shown = gh.visibleAssets();
  return (
    t("main.ghRepoMeta", { releases: gh.releases.value.length, assets: shown }) +
    (shown < total ? t("main.ghRepoHidden", { n: total - shown }) : "")
  );
});

function submit() {
  gh.load(true, false);
}

function onEnter(e) {
  if (e.key === "Enter") {
    e.preventDefault();
    submit();
  }
}

function toggleLatest() {
  gh.load(true, false);
}

function reload() {
  gh.load(true, true);
}

onMounted(() => inputEl.value?.focus());
</script>

<template>
  <div class="gh-scroll">
    <section class="gh-search">
      <label class="gh-label" for="ghRepo">{{ $t("main.ghRepoLabel") }}</label>
      <div class="gh-input-row">
        <input
          id="ghRepo"
          ref="inputEl"
          v-model="gh.input.value"
          class="gh-input"
          type="text"
          spellcheck="false"
          autocomplete="off"
          :placeholder="$t('main.ghRepoPlaceholder')"
          @keydown="onEnter"
        />
        <button id="ghParse" class="btn btn-primary" type="button" :disabled="gh.loading.value" @click="submit">
          {{ gh.loading.value ? $t("main.ghParsing") : $t("main.ghParse") }}
        </button>
      </div>
      <div class="gh-options">
        <label class="gh-check">
          <input v-model="gh.latest.value" type="checkbox" @change="toggleLatest" />
          <span>{{ $t("main.ghLatest") }}</span>
        </label>
        <label class="gh-check">
          <input v-model="gh.onlyDevice.value" type="checkbox" />
          <span>{{ $t("main.ghOnlyDevice") }}</span>
        </label>
        <label class="gh-check">
          <input v-model="gh.cloneEnabled.value" type="checkbox" />
          <span>{{ $t("main.ghParseClone") }}</span>
        </label>
        <span id="ghRate" class="gh-rate" :class="{ warn: rateWarn }">{{ rateText }}</span>
      </div>
      <div id="ghError" class="notice notice-danger" v-show="gh.error.value">{{ gh.error.value }}</div>
    </section>

    <section v-show="!gh.loaded.value" id="ghIntro" class="gh-intro">
      <span class="gh-intro-ico"><Icon name="download" :size="20" /></span>
      <div>
        <div class="gh-intro-title">{{ $t("main.ghIntroTitle") }}</div>
        <div class="gh-intro-text">{{ $t("main.ghIntroText") }}</div>
      </div>
    </section>

    <section v-show="gh.loaded.value && gh.releases.value.length" id="ghResult" class="gh-result">
      <header class="gh-repo">
        <div class="gh-repo-main">
          <span class="gh-repo-name">{{ gh.repo.value || gh.input.value }}</span>
          <span class="gh-repo-meta">{{ repoMeta }}</span>
        </div>
        <button
          id="ghRefresh"
          class="btn btn-ghost gh-refresh"
          type="button"
          :title="$t('main.ghRefreshTitle')"
          :disabled="gh.loading.value"
          @click="reload"
        >
          <Icon name="refresh" :size="14" />{{ $t("main.ghRefresh") }}
        </button>
      </header>

      <div id="ghReleases" class="gh-releases">
        <article v-if="gh.cloneEnabled.value && gh.repo.value && gh.clone.value" class="gh-clone-card">
          <div class="gh-clone-head">
            <span class="gh-clone-title">{{ $t("main.ghCloneTitle") }}</span>
            <span class="gh-clone-url">{{ gh.clone.value.raw }}</span>
          </div>
          <div class="gh-clone-actions">
            <button
              type="button"
              class="btn gh-copy-clone"
              :title="$t('main.ghCopyCloneTitle')"
              @click="gh.copyClone()"
            >
              <Icon name="copy" :size="14" />{{ $t("main.ghCopyClone") }}
            </button>
            <button
              type="button"
              class="btn btn-primary gh-clone-run"
              :disabled="gh.cloneBusy.value"
              @click="gh.cloneRepo()"
            >
              {{ gh.cloneBusy.value ? $t("main.ghCloningBtn") : $t("main.ghCloneRun") }}
            </button>
          </div>
          <div v-if="gh.cloneResult.value" class="gh-clone-result">
            <span>{{ $t("main.ghClonedTo", { path: gh.cloneResult.value }) }}</span>
            <button type="button" class="btn" @click="gh.openCloneDir()">
              {{ $t("main.ghOpenDir") }}
            </button>
          </div>
          <div v-else class="gh-note">
            {{ gh.cloneBusy.value ? $t("main.ghCloneBusyNote") : $t("main.ghCloneNote") }}
          </div>
        </article>

        <article
          v-for="(rel, idx) in gh.releases.value"
          :key="rel.tag"
          class="gh-release"
          :class="{ open: idx === 0 || gh.openTags.has(rel.tag) }"
        >
          <button type="button" class="gh-rel-head" @click="gh.toggleTag(rel.tag)">
            <span class="gh-tag">{{ rel.tag }}</span>
            <span v-if="rel.name && rel.name !== rel.tag" class="gh-rel-name">{{ rel.name }}</span>
            <span class="gh-badges">
              <span v-if="idx === 0" class="gh-badge latest">{{ $t("main.ghBadgeLatest") }}</span>
              <span v-if="rel.prerelease" class="gh-badge pre">{{ $t("main.ghBadgePre") }}</span>
            </span>
            <span class="gh-date">{{ rel.publishedAt || "" }}</span>
            <span class="gh-chev"><Icon name="chevron-down" :size="14" /></span>
          </button>

          <div class="gh-assets">
            <template v-if="gh.shownAssets(rel).length">
              <div
                v-for="asset in gh.shownAssets(rel)"
                :key="asset.name"
                class="gh-asset"
                :class="{ dim: !asset.fits }"
              >
                <span class="gh-file" :title="asset.name">{{ asset.name }}</span>
                <span class="gh-kind" :class="asset.platform || 'unknown'">
                  {{ gh.platformText(asset.platform) }}
                </span>
                <span class="gh-meta">
                  {{ fmtBytes(asset.size) }}
                  <template v-if="asset.downloads"> · ↓{{ fmtCount(asset.downloads) }}</template>
                </span>
                <button
                  type="button"
                  class="btn btn-primary gh-get"
                  @click="gh.downloadAsset(asset, $event.currentTarget)"
                >
                  {{ $t("main.ghDownload") }}
                </button>
              </div>
            </template>
            <div v-else class="gh-note">
              {{ $t("main.ghNoMatch") }}
              <template v-if="rel.hiddenAssets">
                {{ $t("main.ghHiddenAssets", { n: rel.hiddenAssets }) }}
              </template>
            </div>
            <div v-if="gh.onlyDevice.value && rel.hiddenAssets > 0 && gh.shownAssets(rel).length" class="gh-note">
              {{ $t("main.ghHiddenNote", { n: rel.hiddenAssets }) }}
            </div>
          </div>
        </article>
      </div>

      <div class="gh-more-wrap">
        <button
          id="ghMore"
          class="btn"
          type="button"
          v-show="gh.hasMore.value"
          :disabled="gh.loading.value"
          @click="gh.load(false, false)"
        >
          {{ $t("main.ghMore") }}
        </button>
      </div>
    </section>

    <div v-show="gh.loaded.value && !gh.releases.value.length" id="ghEmpty" class="gh-empty">
      <span class="gh-intro-ico"><Icon name="search" :size="20" /></span>
      <div>{{ $t("main.ghEmpty") }}</div>
    </div>
  </div>
</template>
