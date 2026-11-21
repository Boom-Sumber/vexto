// 统一轻提示（会话 99）：标题栏下方居中，最多同时 3 条；普通 3 秒、错误 6 秒，
// 悬停暂停、点击关闭。设置页 / 任务列表共用。
//
// 会话 105 增加「常驻加载态」：kind=loading 时显示旋转圆环、不自动消失；
// 返回的 handle.update() 可原地改成成功 / 失败，避免 clone 期间再叠一条。
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

  function iconNode(kind) {
    if (kind === "loading") {
      const s = document.createElement("span");
      s.className = "vx-spinner";
      s.setAttribute("aria-hidden", "true");
      return s;
    }
    const s = document.createElement("span");
    s.className = "vx-toast-ico";
    if (window.Icons) {
      const name = kind === "error" ? "alert" : kind === "success" ? "check" : "info";
      s.innerHTML = Icons.svg(name, 16);
    }
    return s;
  }

  function show(msg, opts) {
    if (!msg) return null;
    let current = Object.assign({}, opts || {});
    let closed = false;
    const h = ensureHost();
    while (h.children.length >= 3) h.firstElementChild.remove();

    const toast = document.createElement("div");
    toast.className = "vx-toast";
    const text = document.createElement("span");
    text.textContent = msg;
    toast.appendChild(text);

    const kind = () =>
      current.kind === "error" ||
      current.kind === "success" ||
      current.kind === "loading"
        ? current.kind
        : "info";

    const applyKind = () => {
      const k = kind();
      // 不能重写 className：那会把入场态 `in` 一起抹掉，update 后 toast
      // 会突然变成透明。只替换 kind 类。
      toast.classList.remove("info", "error", "success", "loading");
      toast.classList.add(k);
      toast.setAttribute("role", k === "error" ? "alert" : "status");
      const old = toast.querySelector(".vx-toast-ico, .vx-spinner");
      if (old) old.remove();
      toast.insertBefore(iconNode(k), text);
    };

    let timer = 0;
    const close = () => {
      if (closed) return;
      closed = true;
      clearTimeout(timer);
      toast.classList.remove("in");
      setTimeout(() => toast.remove(), 180);
    };

    const arm = () => {
      clearTimeout(timer);
      if (closed) return;
      const d =
        typeof current.duration === "number"
          ? current.duration
          : kind() === "error"
            ? 6000
            : kind() === "success"
              ? 2000
              : kind() === "loading"
                ? 0
                : 3000;
      if (d > 0) timer = setTimeout(close, d);
    };

    const handle = {
      close,
      update(nextMsg, nextOpts) {
        if (closed) return handle;
        if (nextMsg) text.textContent = nextMsg;
        if (nextOpts) {
          current = Object.assign({}, current, nextOpts);
          // 从 loading 切成 error 时不能继承 duration:0，否则错误提示永不消失。
          if (!Object.prototype.hasOwnProperty.call(nextOpts, "duration")) {
            delete current.duration;
          }
        }
        applyKind();
        arm();
        return handle;
      },
    };

    toast.addEventListener("mouseenter", () => clearTimeout(timer));
    toast.addEventListener("mouseleave", arm);
    toast.addEventListener("click", close);

    applyKind();
    h.appendChild(toast);
    requestAnimationFrame(() => toast.classList.add("in"));
    arm();
    return handle;
  }

  function loading(msg) {
    return show(msg, { kind: "loading", duration: 0 });
  }

  window.Toast = { show, loading };
})();
