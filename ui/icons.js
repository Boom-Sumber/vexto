// 图标库：所有自绘图标只在这里定义，页面用 Icons.svg(name, size) 取用。
//
// 约定（见 docs/13-UI-DESIGN-SYSTEM.md §13.5）：
// - 统一 24×24 viewBox、stroke 1.6、round caps/joins；
// - 颜色一律 currentColor，跟着父元素文字色走；
// - 尺寸只用 14 / 16 / 20 三档（标题栏 14，正文 16，强调 20）；
// - 不写 fill，需要填充的单独标注。

window.Icons = (function () {
  const P = {
    // 窗口控制
    minimize: '<path d="M5 12h14"/>',
    maximize: '<rect x="5" y="5" width="14" height="14" rx="2"/>',
    restore:
      '<rect x="4" y="8" width="12" height="12" rx="2"/>' +
      '<path d="M8 4h10a2 2 0 0 1 2 2v10"/>',
    close: '<path d="M6 6l12 12M18 6L6 18"/>',

    // 通用
    check: '<path d="M5 13l4 4L19 7"/>',
    plus: '<path d="M12 5v14M5 12h14"/>',
    trash:
      '<path d="M4 7h16M9 7V5h6v2"/>' +
      '<path d="M6.5 7l1 12.5h9l1-12.5"/>',
    refresh: '<path d="M20 12a8 8 0 1 1-2.3-5.6"/><path d="M20 4v5h-5"/>',
    search: '<circle cx="11" cy="11" r="6"/><path d="M16 16l4 4"/>',
    external:
      '<path d="M14 4h6v6"/><path d="M20 4l-9 9"/>' +
      '<path d="M18 14v4a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4"/>',
    info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 7.5h.01"/>',
    // 行内「更多」菜单入口。按 §13.5 不许用 ⋯ 字符
    more:
      '<circle cx="6" cy="12" r="1.4"/>' +
      '<circle cx="12" cy="12" r="1.4"/>' +
      '<circle cx="18" cy="12" r="1.4"/>',
    help:
      '<circle cx="12" cy="12" r="9"/>' +
      '<path d="M9.5 9a2.5 2.5 0 1 1 3.7 2.2c-.8.4-1.2 1-1.2 1.8v.5"/>' +
      '<path d="M12 17h.01"/>',
    alert: '<path d="M12 4l9 16H3z"/><path d="M12 10v4M12 17h.01"/>',
    gear:
      '<circle cx="12" cy="12" r="3"/>' +
      '<path d="M12 3v2M12 19v2M3 12h2M19 12h2' +
      'M5.6 5.6l1.4 1.4M17 17l1.4 1.4M18.4 5.6L17 7M7 17l-1.4 1.4"/>',

    // 文件 / 下载
    folder:
      '<path d="M3 7.5A1.5 1.5 0 0 1 4.5 6h4l2 2h7A1.5 1.5 0 0 1 19 9.5v7' +
      'A1.5 1.5 0 0 1 17.5 18h-13A1.5 1.5 0 0 1 3 16.5z"/>',
    clock: '<circle cx="12" cy="12" r="8"/><path d="M12 7.5V12l3 2"/>',
    download: '<path d="M12 4v11"/><path d="M7 11l5 5 5-5"/><path d="M5 20h14"/>',
    // 发送方向（与 download 镜像）；用在传输进度行的方向标识上
    upload: '<path d="M12 20V9"/><path d="M7 13l5-5 5 5"/><path d="M5 4h14"/>',
    transfer:
      '<path d="M7 4v13"/><path d="M4 14l3 3 3-3"/>' +
      '<path d="M17 20V7"/><path d="M14 10l3-3 3 3"/>',
    file:
      '<path d="M6 3h7l5 5v13a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z"/>' +
      '<path d="M13 3v5h5"/>',

    // 设备类型（接收确认框 / 设备列表）
    "device-phone":
      '<rect x="6" y="3" width="12" height="18" rx="2"/>' +
      '<path d="M12 17.5h.01"/>',
    "device-desktop":
      '<rect x="3" y="4" width="18" height="12" rx="2"/>' +
      '<path d="M8 20h8M12 16v4"/>',
    "device-web":
      '<circle cx="12" cy="12" r="8"/>' +
      '<path d="M4 12h16"/>' +
      '<path d="M12 4a12 12 0 0 1 3 8 12 12 0 0 1-3 8 12 12 0 0 1-3-8 12 12 0 0 1 3-8z"/>',
    "device-server":
      '<rect x="3" y="4" width="18" height="6" rx="2"/>' +
      '<rect x="3" y="14" width="18" height="6" rx="2"/>' +
      '<path d="M7 7h.01M7 17h.01"/>',
    "device-headless":
      '<path d="M4 17l6-6-6-6"/><path d="M12 19h8"/>',

    // 播放控制
    pause:
      '<rect x="8" y="5" width="3.2" height="14" rx="1"/>' +
      '<rect x="12.8" y="5" width="3.2" height="14" rx="1"/>',
    play: '<path d="M8 5l11 7-11 7z"/>',

    // 翻译弹窗
    pin:
      '<path d="M9.5 3.5h5l-.8 5.2 2.8 2.8H7.5l2.8-2.8z"/>' +
      '<path d="M12 11.5V20"/>',
    copy:
      '<rect x="9" y="9" width="11" height="11" rx="2"/>' +
      '<path d="M5 15V6a2 2 0 0 1 2-2h9"/>',

    // 方向
    "arrow-left": '<path d="M19 12H5"/><path d="M11 6l-6 6 6 6"/>',
    "arrow-right": '<path d="M5 12h14"/><path d="M13 6l6 6-6 6"/>',
    "chevron-down": '<path d="M6 9l6 6 6-6"/>',
    sort:
      '<path d="M8 6v12M8 18l-3-3M8 18l3-3"/>' +
      '<path d="M16 18V6M16 6l-3 3M16 6l3 3"/>',

    // 导航（UI v2 侧栏：设置 / 翻译 / 下载分类 / 互传 / 大模型）
    globe:
      '<circle cx="12" cy="12" r="9"/>' +
      '<path d="M3 12h18"/>' +
      '<path d="M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/>',
    grid:
      '<rect x="3" y="3" width="7" height="7" rx="1.5"/>' +
      '<rect x="14" y="3" width="7" height="7" rx="1.5"/>' +
      '<rect x="3" y="14" width="7" height="7" rx="1.5"/>' +
      '<rect x="14" y="14" width="7" height="7" rx="1.5"/>',
    "check-circle": '<circle cx="12" cy="12" r="9"/><path d="M8.5 12.5l2.5 2.5 5-5"/>',
    send:
      '<path d="M22 2 11 13"/>' +
      '<path d="M22 2 15 22l-4-9-9-4z"/>',
    bolt: '<path d="M13 2 3 14h9l-1 8 10-12h-9z"/>',
  };

  // 设计稿给的两个「实心/彩色」图标：viewBox 是 1024，颜色写死或跟 currentColor，
  // 不套统一的 24×24 stroke 模板（见 §13.5）。用 Icons.raw(name, size) 取用。
  const RAW = {
    // 粉红圆底 + 白色 ×：文件卡片右上角删除
    deleteRound:
      '<circle cx="512" cy="512" r="512" fill="#FF6483"/>' +
      '<path d="M727.04 733.730133a38.331733 38.331733 0 0 1-54.033067 0l-162.133333-162.235733-162.0992 162.133333a38.229333 38.229333 0 1 1-54.101333-54.033066l162.133333-162.133334-159.914667-159.914666a38.229333 38.229333 0 0 1 54.101334-54.033067l159.914666 159.914667 159.914667-159.914667a38.229333 38.229333 0 0 1 54.033067 54.033067l-159.914667 159.914666 162.133333 162.133334a38.331733 38.331733 0 0 1 0 54.033066v0.1024z" fill="#FFFFFF"/>',
    // 向下箭头 + 底线：拖放页中间的大图标，颜色跟 currentColor
    bigDownload:
      '<path d="M960.512 897.024H63.488c-34.816 0-63.488 28.672-63.488 63.488C0 995.328 28.672 1024 63.488 1024h897.024c34.816 0 63.488-28.672 63.488-63.488 0-36.864-28.672-63.488-63.488-63.488z m-493.568-133.12c24.576 24.576 65.536 24.576 90.112 0L829.44 491.52c24.576-24.576 24.576-65.536 0-90.112-24.576-24.576-65.536-24.576-90.112 0l-161.792 161.792V63.488C575.488 28.672 546.816 0 512 0c-34.816 0-63.488 28.672-63.488 63.488v501.76L286.72 403.456c-24.576-24.576-65.536-24.576-90.112 0-24.576 24.576-24.576 65.536 0 90.112l270.336 270.336z"/>',
  };

  /// 生成一个 SVG 字符串。`size` 是像素边长，只用 14 / 16 / 20。
  function svg(name, size) {
    const body = P[name] || P.info;
    const s = size || 16;
    return (
      `<svg viewBox="0 0 24 24" width="${s}" height="${s}" fill="none" ` +
      `stroke="currentColor" stroke-width="1.6" stroke-linecap="round" ` +
      `stroke-linejoin="round" aria-hidden="true" class="ico-svg">${body}</svg>`
    );
  }

  /// 把一个元素的内容换成指定图标（返回该元素，方便链式调用）。
  function fill(el, name, size) {
    if (!el) return el;
    el.innerHTML = svg(name, size);
    el.classList.add("ico");
    return el;
  }

  /// 设计稿里的实心图标：自带 viewBox 1024，颜色由图标自己决定或跟 currentColor。
  function raw(name, size) {
    const s = size || 24;
    const body = RAW[name] || RAW.bigDownload;
    const fill = name === "deleteRound" ? "none" : "currentColor";
    return (
      `<svg viewBox="0 0 1024 1024" width="${s}" height="${s}" fill="${fill}" ` +
      `aria-hidden="true" class="ico-svg">${body}</svg>`
    );
  }

  function has(name) {
    return Object.prototype.hasOwnProperty.call(P, name) || Object.prototype.hasOwnProperty.call(RAW, name);
  }

  return { svg, raw, fill, has, names: () => Object.keys(P).concat(Object.keys(RAW)) };
})();
