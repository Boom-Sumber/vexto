// GitHub 加速下载视图：数据来自后端 `github_releases`。
//
// 分类里的「GitHub 加速」不是任务过滤器，而是独立视图：main.js 负责在
// 任务列表和本视图之间切换，本文件只处理解析与展示。下载按钮走
// `github_open_task`，预填现有新建任务页，不在前端直接调下载引擎。

window.GithubView = (function () {
  const T = window.__TAURI__;
  const invoke = T.core.invoke;
  const el = (id) => document.getElementById(id);

  const state = {
    inited: false,
    loading: false,
    loaded: false,
    all: false,
    page: 0,
    hasMore: false,
    repo: "",
    input: "",
    releases: [],
    rate: null,
    openTags: new Set(),
  };

  function errText(e) {
    if (e == null) return "未知错误";
    if (typeof e === "string") return e;
    if (e.message) return String(e.message);
    return String(e);
  }

  function fmtBytes(n) {
    n = Number(n) || 0;
    if (n < 1024) return n + " B";
    const u = ["KB", "MB", "GB", "TB"];
    let i = -1;
    do {
      n /= 1024;
      i++;
    } while (n >= 1024 && i < u.length - 1);
    return n.toFixed(n < 10 ? 1 : 0) + " " + u[i];
  }

  function fmtCount(n) {
    n = Number(n) || 0;
    if (n >= 1e6) return (n / 1e6).toFixed(1).replace(/\.0$/, "") + "M";
    if (n >= 1e3) return (n / 1e3).toFixed(1).replace(/\.0$/, "") + "k";
    return String(n);
  }

  function span(cls, text) {
    const s = document.createElement("span");
    if (cls) s.className = cls;
    if (text != null) s.textContent = text;
    return s;
  }

  function showError(msg) {
    const box = el("ghError");
    box.textContent = msg || "";
    box.classList.toggle("hidden", !msg);
  }

  function platformText(p) {
    return {
      windows: "Win x64",
      unknown: "未标注",
      other: "其他平台",
      aux: "校验文件",
    }[p] || "未标注";
  }

  function renderRate() {
    const r = el("ghRate");
    if (state.loading) {
      r.textContent = "正在连接 GitHub…";
      r.classList.remove("warn");
      return;
    }
    if (!state.rate || state.rate.remaining == null) {
      r.textContent = "";
      r.classList.remove("warn");
      return;
    }
    const limit = state.rate.limit || 60;
    r.textContent =
      "接口限额 " + state.rate.remaining + "/" + limit +
      (state.rate.cached ? " · 缓存" : "");
    r.classList.toggle("warn", state.rate.remaining <= Math.min(5, limit));
  }

  function setLoading(on) {
    state.loading = on;
    el("ghParse").disabled = on;
    el("ghParse").textContent = on ? "解析中…" : "解析";
    el("ghRefresh").disabled = on;
    el("ghLatest").disabled = on;
    el("ghDevice").disabled = on;
    el("ghMore").disabled = on;
    renderRate();
  }

  async function load(reset, refresh) {
    if (state.loading) return;
    const input = el("ghRepo").value.trim();
    if (!input) {
      showError("请输入 GitHub 仓库地址");
      el("ghRepo").focus();
      return;
    }
    if (reset) {
      state.input = input;
      state.repo = "";
      state.page = 1;
      state.releases = [];
      state.hasMore = false;
      state.openTags.clear();
      state.rate = null;
      state.loaded = false;
      el("ghReleases").replaceChildren();
      el("ghResult").classList.add("hidden");
    } else {
      state.page += 1;
    }
    state.all = !el("ghLatest").checked;
    showError("");
    setLoading(true);
    render();

    try {
      const r = await invoke("github_releases", {
        repo: state.input,
        all: state.all,
        page: state.page,
        refresh: !!refresh,
      });
      state.releases = reset
        ? (r.releases || [])
        : state.releases.concat(r.releases || []);
      state.repo = r.repo || state.input;
      state.page = r.page || state.page;
      state.hasMore = !!r.hasMore;
      state.rate = {
        limit: r.rateLimit || null,
        remaining: r.rateRemaining == null ? null : r.rateRemaining,
        reset: r.rateReset || null,
        cached: !!r.cached,
      };
      state.loaded = true;
      render();
      if (r.message) window.Toast?.show?.(r.message);
    } catch (e) {
      if (!reset) state.page = Math.max(1, state.page - 1);
      showError(errText(e));
      if (reset) {
        state.releases = [];
        state.loaded = false;
        render();
      }
    } finally {
      setLoading(false);
    }
  }

  function render() {
    const has = state.releases.length > 0;
    el("ghIntro").classList.toggle("hidden", state.loaded);
    el("ghEmpty").classList.toggle("hidden", !state.loaded || has);
    el("ghResult").classList.toggle("hidden", !has);
    el("ghRepoName").textContent = state.repo || state.input;
    const device = el("ghDevice").checked;
    const totalAssets = state.releases.reduce((n, r) => n + r.assets.length, 0);
    const shownAssets = state.releases.reduce(
      (n, r) => n + (device ? r.assets.filter((a) => a.fits).length : r.assets.length),
      0
    );
    el("ghRepoMeta").textContent = has
      ? state.releases.length + " 个版本 · " + shownAssets + " 个资源" +
        (shownAssets < totalAssets ? "（隐藏 " + (totalAssets - shownAssets) + "）" : "")
      : "";
    renderRate();

    const box = el("ghReleases");
    box.replaceChildren();
    state.releases.forEach((rel, i) => box.appendChild(releaseNode(rel, i)));
    el("ghMore").classList.toggle("hidden", !has || !state.hasMore);
  }

  function releaseNode(rel, idx) {
    const card = document.createElement("article");
    card.className =
      "gh-release" + (idx === 0 || state.openTags.has(rel.tag) ? " open" : "");

    const head = document.createElement("button");
    head.type = "button";
    head.className = "gh-rel-head";

    head.appendChild(span("gh-tag", rel.tag));
    if (rel.name && rel.name !== rel.tag) {
      head.appendChild(span("gh-rel-name", rel.name));
    }
    const badges = document.createElement("span");
    badges.className = "gh-badges";
    if (idx === 0) badges.appendChild(span("gh-badge latest", "最新"));
    if (rel.prerelease) badges.appendChild(span("gh-badge pre", "预发布"));
    head.appendChild(badges);
    head.appendChild(span("gh-date", rel.publishedAt || ""));

    const chev = span("gh-chev");
    if (window.Icons) chev.innerHTML = Icons.svg("chevron-down", 14);
    head.appendChild(chev);

    head.addEventListener("click", () => {
      const open = card.classList.toggle("open");
      if (open) state.openTags.add(rel.tag);
      else state.openTags.delete(rel.tag);
    });

    const body = document.createElement("div");
    body.className = "gh-assets";
    const device = el("ghDevice").checked;
    const shown = (rel.assets || []).filter((a) => !device || a.fits);
    if (shown.length) {
      for (const a of shown) body.appendChild(assetNode(a));
    } else {
      body.appendChild(
        note(
          "没有匹配本设备的文件" +
            (rel.hiddenAssets ? "（已隐藏 " + rel.hiddenAssets + " 个）" : "")
        )
      );
    }
    if (device && rel.hiddenAssets > 0 && shown.length > 0) {
      body.appendChild(note("已隐藏 " + rel.hiddenAssets + " 个非本设备文件"));
    }

    card.append(head, body);
    return card;
  }

  function note(text) {
    const n = document.createElement("div");
    n.className = "gh-note";
    n.textContent = text;
    return n;
  }

  function assetNode(a) {
    const row = document.createElement("div");
    row.className = "gh-asset" + (a.fits ? "" : " dim");

    const name = span("gh-file", a.name);
    name.title = a.name;
    const kind = span("gh-kind " + (a.platform || "unknown"), platformText(a.platform));
    const meta = span(
      "gh-meta",
      fmtBytes(a.size) + (a.downloads ? " · ↓" + fmtCount(a.downloads) : "")
    );

    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "btn btn-primary gh-get";
    btn.textContent = "下载";
    btn.addEventListener("click", () => download(a, btn));

    row.append(name, kind, meta, btn);
    return row;
  }

  async function download(a, btn) {
    btn.disabled = true;
    try {
      await invoke("github_open_task", {
        url: a.url,
        filename: a.name,
        size: Number(a.size) || 0,
      });
      window.Toast?.show?.("已打开新建任务：" + a.name);
    } catch (e) {
      showError(errText(e));
    } finally {
      btn.disabled = false;
    }
  }

  function init() {
    if (state.inited) return;
    state.inited = true;
    el("ghParse").addEventListener("click", () => load(true, false));
    el("ghRepo").addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        load(true, false);
      }
    });
    el("ghLatest").addEventListener("change", () => load(true, false));
    el("ghDevice").addEventListener("change", render);
    el("ghMore").addEventListener("click", () => load(false, false));
    el("ghRefresh").addEventListener("click", () => load(true, true));
    render();
  }

  function show() {
    init();
    el("ghRepo").focus();
  }

  return { show };
})();
