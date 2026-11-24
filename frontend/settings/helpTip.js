let tip = null;

function ensureTip() {
  if (tip && tip.isConnected) return tip;
  tip = document.createElement("div");
  tip.id = "help-tip";
  document.body.append(tip);
  return tip;
}

export function showHelp(target, text) {
  if (!text) return;
  const el = ensureTip();
  el.textContent = text;
  el.classList.add("on");
  const r = target.getBoundingClientRect();
  const tw = el.offsetWidth;
  const th = el.offsetHeight;
  let left = Math.max(8, Math.min(r.left, window.innerWidth - tw - 8));
  let top = r.bottom + 6;
  if (top + th > window.innerHeight - 8) top = r.top - th - 6;
  el.style.left = left + "px";
  el.style.top = top + "px";
}

export function hideHelp() {
  if (tip) tip.classList.remove("on");
}

window.addEventListener("scroll", hideHelp, true);
