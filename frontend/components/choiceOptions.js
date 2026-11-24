/** 选项归一化与「分段控件还是下拉」的判定。
 *
 *  schema 里 select 的选项是 `[[value, label], ...]`，组件内部统一用对象，
 *  避免每个调用点各写一遍解构。
 */

export function toItems(options) {
  return (options || []).map((o) =>
    Array.isArray(o)
      ? { value: String(o[0]), label: String(o[1]) }
      : { value: String(o.value), label: String(o.label), title: o.title },
  );
}

/* 12px 字号下的估算宽度：汉字按 12px、拉丁按 6.4px 平均。只用来决定
 * 「塞不塞得进分段」，不参与排版——真实溢出仍由 ellipsis 兜底。 */
const CJK_W = 12;
const ASCII_W = 6.4;

function textWidth(text) {
  let w = 0;
  for (const ch of String(text)) w += ch.charCodeAt(0) < 0x2e80 ? ASCII_W : CJK_W;
  return w;
}

/** 选项 ≤3 且最长文案放得下 → 分段控件；否则走下拉。
 *  available 是控件列宽度（设置页 / 弹窗都是 214px）。 */
export function preferSegmented(items, available = 214) {
  if (items.length < 2 || items.length > 3) return false;
  // 减去按钮左右内边距（12）与轨道 / 边框留白（2）
  const per = available / items.length - 14;
  return items.every((it) => textWidth(it.label) <= per);
}
