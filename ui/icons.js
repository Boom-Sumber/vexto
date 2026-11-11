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

  function has(name) {
    return Object.prototype.hasOwnProperty.call(P, name);
  }

  return { svg, fill, has, names: () => Object.keys(P) };
})();
