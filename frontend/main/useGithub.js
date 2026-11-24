import { reactive, ref, shallowRef } from "vue";
import { t } from "../i18n/index.js";
import { errText } from "../settings/utils.js";

// GitHub 加速视图：数据来自后端 `github_releases`。
// 下载直接入队后由调用方切回「全部」；clone 由后端弹目录选择并后台执行。
export function useGithub({ toast, onShowAll }) {
  const invoke = window.__TAURI__.core.invoke;

  const input = ref("");
  const repo = ref("");
  const releases = shallowRef([]);
  const loaded = ref(false);
  const loading = ref(false);
  const hasMore = ref(false);
  const page = ref(0);
  const rate = ref(null);
  const error = ref("");
  const latest = ref(true);
  const onlyDevice = ref(true);
  const cloneEnabled = ref(false);
  const clone = ref(null);
  const cloneBusy = ref(false);
  const cloneResult = ref("");
  const openTags = reactive(new Set());

  function showError(msg) {
    error.value = msg || "";
  }

  function platformText(p) {
    return {
      windows: t("main.ghWin64"),
      unknown: t("main.ghUnknownPlatform"),
      other: t("main.ghOtherPlatform"),
      aux: t("main.ghChecksum"),
    }[p] || t("main.ghUnknownPlatform");
  }

  async function load(reset, refresh) {
    if (loading.value) return;
    const value = input.value.trim();
    if (!value) {
      showError(t("main.ghNeedRepo"));
      return;
    }
    if (reset) {
      repo.value = "";
      page.value = 1;
      releases.value = [];
      hasMore.value = false;
      openTags.clear();
      rate.value = null;
      clone.value = null;
      cloneResult.value = "";
      loaded.value = false;
    } else {
      page.value += 1;
    }
    showError("");
    loading.value = true;

    try {
      const r = await invoke("github_releases", {
        repo: value,
        all: !latest.value,
        page: page.value,
        refresh: !!refresh,
      });
      releases.value = reset
        ? r.releases || []
        : releases.value.concat(r.releases || []);
      repo.value = r.repo || value;
      page.value = r.page || page.value;
      hasMore.value = !!r.hasMore;
      clone.value = r.clone || null;
      rate.value = {
        limit: r.rateLimit || null,
        remaining: r.rateRemaining == null ? null : r.rateRemaining,
        cached: !!r.cached,
      };
      loaded.value = true;
      if (r.message) toast.show(r.message);
    } catch (e) {
      if (!reset) page.value = Math.max(1, page.value - 1);
      showError(errText(e));
      if (reset) {
        releases.value = [];
        loaded.value = false;
      }
    } finally {
      loading.value = false;
    }
  }

  function toggleTag(tag) {
    if (openTags.has(tag)) openTags.delete(tag);
    else openTags.add(tag);
  }

  function shownAssets(rel) {
    return (rel.assets || []).filter((a) => !onlyDevice.value || a.fits);
  }

  function totalAssets() {
    return releases.value.reduce((n, r) => n + (r.assets || []).length, 0);
  }

  function visibleAssets() {
    return releases.value.reduce((n, r) => n + shownAssets(r).length, 0);
  }

  async function downloadAsset(asset, btn) {
    if (btn) btn.disabled = true;
    try {
      await invoke("github_download", { url: asset.url, filename: asset.name });
      toast.show(t("main.ghAdded", { name: asset.name }));
      onShowAll();
    } catch (e) {
      showError(errText(e));
    } finally {
      if (btn) btn.disabled = false;
    }
  }

  async function copyClone() {
    const text = clone.value && clone.value.command;
    if (!text) return;
    try {
      await invoke("copy_text", { text });
      toast.show(t("main.ghCloneCopied"));
    } catch {
      try {
        await navigator.clipboard.writeText(text);
        toast.show(t("main.ghCloneCopied"));
      } catch (e2) {
        showError(t("main.ghCopyFailed", { message: errText(e2) }));
      }
    }
  }

  async function cloneRepo() {
    if (cloneBusy.value || !repo.value) return;
    cloneBusy.value = true;
    cloneResult.value = "";
    showError("");
    const handle = toast.loading(t("main.ghCloning", { repo: repo.value }));
    try {
      const r = await invoke("github_clone", { repo: repo.value });
      if (r == null) {
        handle?.close();
        return;
      }
      cloneResult.value = r.path || "";
      handle?.update(t("main.ghCloneDone"), { kind: "success", duration: 2000 });
    } catch (e) {
      handle?.update(t("main.ghCloneFailed", { message: errText(e) }), { kind: "error" });
      showError(errText(e));
    } finally {
      cloneBusy.value = false;
    }
  }

  async function openCloneDir() {
    if (!cloneResult.value) return;
    try {
      await invoke("open_path", { path: cloneResult.value });
    } catch (e) {
      showError(errText(e));
    }
  }

  return {
    input,
    repo,
    releases,
    loaded,
    loading,
    hasMore,
    rate,
    error,
    latest,
    onlyDevice,
    cloneEnabled,
    clone,
    cloneBusy,
    cloneResult,
    openTags,
    load,
    toggleTag,
    shownAssets,
    totalAssets,
    visibleAssets,
    downloadAsset,
    copyClone,
    cloneRepo,
    openCloneDir,
    platformText,
  };
}
