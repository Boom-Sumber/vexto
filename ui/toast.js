// 统一轻提示（会话 99）：标题栏下方居中，最多同时 3 条；普通 3 秒、错误 6 秒，
// 悬停暂停、点击关闭。设置页 / 任务列表共用。
(function () {
  let host = null;

  function ensureHost() {
    if (host && host.isConnected) return host;
    host = document.createElement("div");
    host.className = "vx-toast-host";
    host.setAttribute("role", "status");
    host.setAttribute("aria-live", "polite");
    document.body.appendChild(host);
    return host;
  }

  function show(msg, opts) {
    if (!msg) return null;
    const kind = opts && opts.kind === "error" ? "error" : "info";
    const h = ensureHost();
    while (h.children.length >= 3) h.firstElementChild.remove();

    const toast = document.createElement("div");
    toast.className = "vx-toast " + kind;
    toast.setAttribute("role", kind === "error" ? "alert" : "status");
    if (window.Icons) toast.innerHTML = Icons.svg(kind === "error" ? "alert" : "info", 16);
    const text = document.createElement("span");
    text.textContent = msg;
    toast.appendChild(text);
    h.appendChild(toast);
    requestAnimationFrame(() => toast.classList.add("in"));

    let timer = 0;
    const close = () => {
      clearTimeout(timer);
      toast.classList.remove("in");
      setTimeout(() => toast.remove(), 180);
    };
    const arm = () => {
      clearTimeout(timer);
      const duration =
        opts && typeof opts.duration === "number"
          ? opts.duration
          : kind === "error"
            ? 6000
            : 3000;
      timer = setTimeout(close, duration);
    };
    toast.addEventListener("mouseenter", () => clearTimeout(timer));
    toast.addEventListener("mouseleave", arm);
    toast.addEventListener("click", close);
    arm();
    return { close };
  }

  window.Toast = { show };
})();
