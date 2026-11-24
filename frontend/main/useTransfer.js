import { onBeforeUnmount, reactive, ref, shallowRef } from "vue";
import { locale, t } from "../i18n/index.js";
import { errText } from "../settings/utils.js";
import { fmtBytes, fmtSpeed } from "./format.js";

const DEVICE_ICON = {
  mobile: "device-phone",
  desktop: "device-desktop",
  web: "device-web",
  server: "device-server",
  headless: "device-headless",
};

function deviceIcon(type) {
  return DEVICE_ICON[type] || "device-desktop";
}

function fmtWhen(ms) {
  const d = new Date(ms || 0);
  if (Number.isNaN(d.getTime())) return "—";
  return new Intl.DateTimeFormat(locale.value, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  }).format(d);
}

export function useTransfer({ toast, setView, setSub }) {
  const invoke = window.__TAURI__.core.invoke;
  const listen = window.__TAURI__.event.listen;

  const enabled = ref(true);
  const peers = shallowRef([]);
  const receiveDir = ref("");
  const alias = ref("—");
  const addr = ref("—");
  const autoSave = ref(false);

  const pickPaths = ref([]);
  const pickSizes = ref([]);
  const dragActive = ref(false);
  let dragDepth = 0;

  const pending = shallowRef([]);
  const cfReq = ref(null);
  const cfBusy = ref(false);
  const cfError = ref("");
  let cfTimer = null;

  const sessions = shallowRef([]);
  const tpOpen = ref(false);
  const tpAutoComplete = ref(false);
  const tpPendingCancel = ref(null);
  let dismissed = null;

  const hs = reactive({
    page: 1,
    pageSize: 20,
    total: 0,
    items: [],
    menuOpen: false,
    menuItem: null,
    menuLeft: 8,
    menuTop: 8,
    infoItem: null,
    clearArmed: false,
  });

  let unlisten = [];
  let hsArmTimer = null;

  function notice(msg, danger) {
    if (msg) toast.show(msg, { kind: danger ? "error" : "info" });
  }

  // ── 设备 ────────────────────────────────────────────────────────────

  async function refreshPeers() {
    try {
      peers.value = (await invoke("transfer_peers")) || [];
    } catch (e) {
      notice(t("main.peersFailed", { message: errText(e) }), true);
    }
  }

  async function refreshLocalInfo() {
    try {
      const info = await invoke("transfer_local_info");
      receiveDir.value = info.receive_dir || "";
      alias.value = info.alias || "—";
      const ip = (info.ips && info.ips[0]) || "";
      addr.value = ip
        ? t("main.localAddr", { ip })
        : info.enabled
          ? t("main.noLanAddr")
          : t("main.transferDisabled");
    } catch (e) {
      addr.value = t("main.localInfoFailed", { message: errText(e) });
    }
    await refreshAutoSave();
  }

  async function refreshAutoSave() {
    try {
      const res = await invoke("get_tool_config", { toolId: "transfer" });
      autoSave.value = !!(res && res.values && res.values.transfer_auto_accept === true);
    } catch {
      /* 读不到就当关着 */
    }
  }

  async function toggleAutoSave() {
    const on = !autoSave.value;
    autoSave.value = on;
    try {
      const res = await invoke("save_tool_config", {
        toolId: "transfer",
        values: { transfer_auto_accept: on },
      });
      if (!res || !res.ok) {
        autoSave.value = !on;
        notice((res && res.message) || t("main.saveFailed"), true);
      } else {
        notice(on ? t("main.autoSaveOn") : t("main.autoSaveOff"));
      }
    } catch (e) {
      autoSave.value = !on;
      notice(t("main.saveFailedWith", { message: errText(e) }), true);
    }
  }

  // ── 选文件卡片（S4 · 会话 46） ──────────────────────────────────────

  async function refreshPickSizes() {
    const paths = pickPaths.value || [];
    if (!paths.length) {
      pickSizes.value = [];
      return;
    }
    try {
      pickSizes.value = (await invoke("transfer_path_sizes", { paths })) || [];
    } catch {
      pickSizes.value = [];
    }
  }

  async function addFiles() {
    try {
      const paths = await invoke("transfer_pick_files");
      if (!paths || !paths.length) return;
      for (const p of paths) {
        if (!pickPaths.value.includes(p)) pickPaths.value.push(p);
      }
      pickPaths.value = [...pickPaths.value];
      await refreshPickSizes();
    } catch (e) {
      notice(t("main.pickFailed", { message: errText(e) }), true);
    }
  }

  async function addDropped(paths) {
    if (!paths || !paths.length) return;
    for (const p of paths) {
      if (!pickPaths.value.includes(p)) pickPaths.value.push(p);
    }
    pickPaths.value = [...pickPaths.value];
    await refreshPickSizes();
  }

  function removePick(index) {
    pickPaths.value = pickPaths.value.filter((_, i) => i !== index);
    refreshPickSizes();
  }

  function clearPick() {
    pickPaths.value = [];
    pickSizes.value = [];
  }

  async function sendToPeer(peer) {
    const paths = pickPaths.value.slice();
    if (!paths.length) {
      notice(t("main.pickFirst"));
      return;
    }
    clearPick();
    const target = peer.fingerprint || peer.ip + ":" + peer.port;
    try {
      notice(t("main.sendingFiles", { n: paths.length }));
      await invoke("transfer_send", { target, paths, pin: peer.pin || undefined });
    } catch (e) {
      notice(t("main.sendFailed", { message: errText(e) }), true);
    }
  }

  async function refreshPeersAction() {
    try {
      await invoke("transfer_refresh");
      notice(t("main.refreshingPeers"));
      await refreshPeers();
    } catch (e) {
      notice(t("main.refreshFailed", { message: errText(e) }), true);
    }
  }

  // ── 收到传输请求：互传窗口内的遮罩子对话框（会话 80） ────────────────

  function shortIp(ip) {
    if (!ip) return "#—";
    const parts = String(ip).split(".");
    return "#" + (parts.length > 1 ? parts[parts.length - 1] : ip);
  }

  function closeConfirm(msg) {
    cfReq.value = null;
    cfBusy.value = false;
    cfError.value = "";
    if (cfTimer) {
      clearInterval(cfTimer);
      cfTimer = null;
    }
    if (msg) notice(msg);
  }

  function openConfirm(req) {
    if (!req) return;
    if (cfReq.value && cfReq.value.session_id === req.session_id) return;
    cfReq.value = req;
    cfBusy.value = false;
    cfError.value = "";
    if (!cfTimer) cfTimer = setInterval(pollIncoming, 1200);
    if (!receiveDir.value) refreshLocalInfo();
  }

  async function pollIncoming() {
    const cur = cfReq.value;
    if (!cur || cfBusy.value) return;
    try {
      const list = (await invoke("transfer_pending")) || [];
      if (cfReq.value !== cur || cfBusy.value) return;
      if (!list.length) {
        closeConfirm(t("main.peerCancelled"));
        return;
      }
      if (!list.some((r) => r.session_id === cur.session_id)) {
        openConfirm(list[0]);
      }
    } catch {
      /* 下一轮再试 */
    }
  }

  async function respondIncoming(accept) {
    if (!cfReq.value || cfBusy.value) return;
    cfBusy.value = true;
    cfError.value = "";
    try {
      await invoke("transfer_respond", {
        sessionId: cfReq.value.session_id,
        accept,
      });
      const list = (await invoke("transfer_pending")) || [];
      cfBusy.value = false;
      if (list.length) openConfirm(list[0]);
      else closeConfirm();
    } catch (e) {
      cfBusy.value = false;
      const msg = errText(e);
      if (msg.includes("取消") || msg.includes("过期")) {
        closeConfirm(t("main.requestExpired"));
      } else {
        cfError.value = msg;
      }
    }
  }

  async function refreshPending() {
    try {
      const list = (await invoke("transfer_pending")) || [];
      pending.value = list;
      if (list.length) openConfirm(list[0]);
      else if (cfReq.value && !cfBusy.value) closeConfirm(t("main.peerCancelled"));
    } catch (e) {
      notice(t("main.pendingFailed", { message: errText(e) }), true);
    }
  }

  // ── 传输页（S4 · 会话 45） ───────────────────────────────────────────

  function tpCurrent() {
    return sessions.value.length ? sessions.value[sessions.value.length - 1] : null;
  }

  function tpStatusOf(s) {
    if (!s) return "idle";
    if (s.status === "done") return "done";
    if (s.status === "cancelled") return "cancelled";
    if (s.status === "failed") return "failed";
    return s.status === "preparing" ? "preparing" : "active";
  }

  function tpPct(s) {
    if (!s) return 0;
    if (s.status === "done") return 100;
    if (!s.total) return 0;
    return Math.min(100, Math.round((s.bytes / s.total) * 100));
  }

  function tpFilePct(f) {
    if (!f.total) return f.status === "done" ? 100 : 0;
    return Math.min(100, Math.round((f.bytes / f.total) * 100));
  }

  function refreshSessions() {
    sessions.value = [...sessions.value];
  }

  function tpRender() {
    const s = tpCurrent();
    if (!s) {
      tpOpen.value = false;
      return;
    }
    tpOpen.value = true;
    if (tpStatusOf(s) === "done" && tpAutoComplete && !s._autoClosed) {
      s._autoClosed = true;
      setTimeout(() => {
        if (dismissed === s.session_id) return;
        tpClose();
      }, 1200);
    }
  }

  function tpSync(list) {
    sessions.value = Array.isArray(list) ? list : [];
    tpRender();
  }

  function renderTpWith(s) {
    sessions.value = [s];
    tpRender();
    invoke("transfer_active")
      .then((list) => {
        if (Array.isArray(list) && list.length) tpSync(list);
      })
      .catch(() => {});
  }

  function tpOnProgress(view) {
    if (!view || !view.session_id) return;
    const sid = view.session_id;
    let s = sessions.value.find((x) => x.session_id === sid);
    if (!s) {
      s = {
        session_id: sid,
        direction: view.direction,
        peer: "",
        status: view.status,
        message: view.message || null,
        bytes: 0,
        total: 0,
        speed_bps: null,
        files: [],
      };
      sessions.value = [...sessions.value, s];
      if (sessions.value.length > 4) sessions.value = sessions.value.slice(1);
      if (dismissed !== sid) {
        dismissed = null;
        renderTpWith(s);
        return;
      }
    }
    const sessionLevel = !view.file_name || view.file_name.endsWith(" 个文件");
    if (sessionLevel) {
      if (view.status) s.status = view.status;
      if (view.message) s.message = view.message;
      if (view.speed_bps) s.speed_bps = view.speed_bps;
      if (view.total > 0) {
        s.total = view.total;
        s.bytes = view.bytes;
      }
    } else {
      if (view.speed_bps) s.speed_bps = view.speed_bps;
      if (view.message && !s.message) s.message = view.message;
      let f = s.files.find((x) => x.name === view.file_name);
      if (!f) {
        f = {
          name: view.file_name,
          bytes: view.bytes,
          total: view.total,
          status: view.status,
        };
        s.files.push(f);
      } else {
        f.bytes = view.bytes;
        if (view.total > 0) f.total = view.total;
        if (["done", "failed", "cancelled"].includes(view.status)) f.status = view.status;
      }
      if (s.status !== "cancelled" && s.status !== "failed") {
        s.total = s.files.reduce((a, x) => a + (x.total || 0), 0);
        s.bytes = s.files.reduce((a, x) => a + (x.bytes || 0), 0);
        if (s.files.length && s.files.every((x) => x.status === "done")) s.status = "done";
      }
    }

    const now = Date.now();
    if (s._lastTs && now - s._lastTs > 200) {
      const db = s.bytes - (s._lastBytes || 0);
      if (db >= 0) {
        const inst = (db / (now - s._lastTs)) * 1000;
        s._speed = s._speed ? s._speed * 0.6 + inst * 0.4 : inst;
        s.speed_bps = Math.round(s._speed);
      }
    }
    s._lastTs = now;
    s._lastBytes = s.bytes;

    if (tpOpen.value) {
      refreshSessions();
      tpRender();
    }
  }

  async function tpLoad() {
    try {
      invoke("transfer_local_info")
        .then((info) => {
          if (info && info.receive_dir) {
            receiveDir.value = info.receive_dir;
            if (tpOpen.value) tpRender();
          }
        })
        .catch(() => {});
      const list = await invoke("transfer_active");
      const active = (list || []).some(
        (s) => s.status === "active" || s.status === "preparing",
      );
      sessions.value = list || [];
      if (active) tpRender();
    } catch {
      /* 模块停用 / 服务没起 */
    }
    try {
      const res = await invoke("get_tool_config", { toolId: "transfer" });
      tpAutoComplete.value = !!(res && res.values && res.values.transfer_auto_complete === true);
    } catch {
      tpAutoComplete.value = false;
    }
  }

  async function tpDoCancel(s) {
    try {
      if (s.direction === "send") {
        await invoke("transfer_cancel_send", { sessionId: s.session_id });
      } else {
        await invoke("transfer_cancel_receive", { sessionId: s.session_id });
      }
    } catch (e) {
      notice(t("main.cancelFailed", { message: errText(e) }), true);
    }
  }

  function tpClose() {
    const s = tpCurrent();
    if (s) dismissed = s.session_id;
    tpOpen.value = false;
    tpPendingCancel.value = null;
    refreshSessions();
  }

  function tpAction() {
    const s = tpCurrent();
    if (!s) {
      tpClose();
      return;
    }
    const st = tpStatusOf(s);
    if (st === "active" || st === "preparing") tpPendingCancel.value = s.session_id;
    else tpClose();
  }

  function tpConfirmCancel() {
    const s = tpCurrent();
    tpPendingCancel.value = null;
    if (s) tpDoCancel(s);
  }

  // ── 历史记录（S4 · 会话 45） ─────────────────────────────────────────

  async function hsLoad(page) {
    if (page) hs.page = page;
    try {
      const r = await invoke("transfer_history_page", {
        page: hs.page,
        pageSize: hs.pageSize,
      });
      hs.items = (r && r.items) || [];
      hs.total = (r && r.total) || 0;
      hs.page = (r && r.page) || 1;
    } catch (e) {
      notice(t("main.historyReadFailed", { message: errText(e) }), true);
    }
  }

  function hsDirOf(p) {
    return String(p || "").replace(/[\\/][^\\/]*$/, "");
  }

  function hsStatusText(s) {
    if (s === "done") return t("main.statusDone");
    if (s === "failed") return t("main.statusFailed");
    if (s === "cancelled") return t("main.statusCancelled");
    return s || "—";
  }

  function openHistoryMenu(item, anchor) {
    if (hs.menuOpen && hs.menuItem && hs.menuItem.id === item.id) {
      closeHistoryMenu();
      return;
    }
    hs.menuItem = item;
    const r = anchor.getBoundingClientRect();
    const mw = 184;
    const mh = 160;
    let left = r.right - mw;
    let top = r.bottom + 4;
    if (left < 8) left = 8;
    if (top + mh > window.innerHeight - 8) top = Math.max(8, r.top - mh - 4);
    hs.menuLeft = left;
    hs.menuTop = top;
    hs.menuOpen = true;
  }

  function closeHistoryMenu() {
    hs.menuOpen = false;
    hs.menuItem = null;
  }

  async function hsRemove(item) {
    try {
      await invoke("transfer_history_remove", { id: item.id });
      notice(t("main.historyRemoved"));
      await hsLoad();
    } catch (e) {
      notice(t("main.historyRemoveFailed", { message: errText(e) }), true);
    }
  }

  function onClearHistory() {
    if (!hs.clearArmed) {
      hs.clearArmed = true;
      hsArmTimer = setTimeout(() => {
        hs.clearArmed = false;
        hsArmTimer = null;
      }, 3000);
      return;
    }
    clearTimeout(hsArmTimer);
    hsArmTimer = null;
    hs.clearArmed = false;
    hsClear();
  }

  async function hsClear() {
    try {
      await invoke("transfer_history_clear");
      notice(t("main.historyCleared"));
      await hsLoad(1);
    } catch (e) {
      notice(t("main.historyClearFailed", { message: errText(e) }), true);
    }
  }

  async function openHistoryPath(item, reveal) {
    const first = (item.files || [])[0] || {};
    if (!first.path) return;
    try {
      await invoke("open_path", { path: reveal ? hsDirOf(first.path) : first.path });
    } catch (e) {
      notice(errText(e), true);
    }
  }

  function showHistoryInfo(item) {
    hs.infoItem = item;
  }

  function closeHistoryInfo() {
    hs.infoItem = null;
  }

  // ── 拖放 ────────────────────────────────────────────────────────────

  function draggingFiles(e) {
    const types = e.dataTransfer && e.dataTransfer.types;
    return !!types && Array.from(types).includes("Files");
  }

  function onDragEnter(e) {
    if (!draggingFiles(e)) return;
    e.preventDefault();
    dragDepth += 1;
    dragActive.value = true;
  }

  function onDragOver(e) {
    if (!draggingFiles(e)) return;
    e.preventDefault();
    dragActive.value = true;
  }

  function onDragLeave(e) {
    if (!draggingFiles(e)) return;
    dragDepth = Math.max(0, dragDepth - 1);
    if (dragDepth === 0) dragActive.value = false;
  }

  function onDrop(e) {
    e.preventDefault();
    dragDepth = 0;
    dragActive.value = false;
  }

  // ── 初始化 ──────────────────────────────────────────────────────────
  // 数据面：peers / pending 拉全量，transfer:event 推增量
  // （风险 71：事件只是通知，丢了不影响正确性）。

  async function onSub(sub) {
    if (sub === "receive") {
      await refreshPending();
      await refreshLocalInfo();
    } else if (sub === "history") {
      await hsLoad(1);
    } else if (sub === "send") {
      await refreshPeers();
    }
  }

  async function init() {
    // 模块停用时隐藏「互传」入口（S4.8）
    try {
      const tools = await invoke("list_tools");
      const item = (tools || []).find((x) => x.id === "transfer");
      if (item && !item.enabled) enabled.value = false;
    } catch {
      /* 拿不到就照常显示 */
    }

    try {
      const v = await invoke("transfer_take_view");
      if (v === "transfer") {
        setView("transfer");
        setSub("send");
      }
    } catch {
      /* 无预选 */
    }

    try {
      unlisten.push(
        await listen("transfer:event", (ev) => {
          const e = ev.payload || {};
          if (e.type === "peers") {
            peers.value = e.peers || [];
          } else if (e.type === "incoming") {
            // 收到请求：切到互传页，确认框是它上面的遮罩子对话框（会话 80）
            setView("transfer");
            setSub("receive");
          } else if (e.type === "progress") {
            tpOnProgress(e.view);
          } else if (e.type === "notice") {
            notice(e.message, false);
          }
        }),
      );
      unlisten.push(
        await listen("transfer:dropped", async () => {
          const paths = await invoke("transfer_take_drop");
          if (!paths || !paths.length) return;
          setView("transfer");
          setSub("send");
          dragActive.value = false;
          await addDropped(paths);
        }),
      );
      unlisten.push(
        await listen("main:switch-view", (ev) => {
          if (ev.payload === "transfer") {
            setView("transfer");
            setSub("send");
          }
        }),
      );
    } catch (e) {
      notice(t("main.subscribeTransferFailed", { message: errText(e) }), true);
    }

    document.addEventListener("dragenter", onDragEnter);
    document.addEventListener("dragover", onDragOver);
    document.addEventListener("dragleave", onDragLeave);
    document.addEventListener("drop", onDrop);

    await tpLoad();
    await refreshPeers();
    await refreshPending();
    await refreshLocalInfo();
  }

  function dispose() {
    for (const fn of unlisten) if (typeof fn === "function") fn();
    unlisten = [];
    if (cfTimer) {
      clearInterval(cfTimer);
      cfTimer = null;
    }
    if (hsArmTimer) {
      clearTimeout(hsArmTimer);
      hsArmTimer = null;
    }
    document.removeEventListener("dragenter", onDragEnter);
    document.removeEventListener("dragover", onDragOver);
    document.removeEventListener("dragleave", onDragLeave);
    document.removeEventListener("drop", onDrop);
  }

  onBeforeUnmount(dispose);

  return {
    enabled,
    peers,
    receiveDir,
    alias,
    addr,
    autoSave,
    pickPaths,
    pickSizes,
    dragActive,
    pending,
    cfReq,
    cfBusy,
    cfError,
    sessions,
    tpOpen,
    tpPendingCancel,
    hs,
    deviceIcon,
    shortIp,
    fmtSpeed,
    fmtBytes,
    fmtWhen,
    hsStatusText,
    refreshPeers,
    refreshLocalInfo,
    toggleAutoSave,
    addFiles,
    addDropped,
    removePick,
    clearPick,
    sendToPeer,
    refreshPeersAction,
    respondIncoming,
    refreshPending,
    tpCurrent,
    tpStatusOf,
    tpPct,
    tpFilePct,
    tpAction,
    tpClose,
    tpConfirmCancel,
    tpPendingClear: () => (tpPendingCancel.value = null),
    hsLoad,
    openHistoryMenu,
    closeHistoryMenu,
    hsRemove,
    hsClear,
    onClearHistory,
    openHistoryPath,
    showHistoryInfo,
    closeHistoryInfo,
    onSub,
    init,
  };
}
