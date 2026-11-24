import { locale, t } from "../i18n/index.js";

export function stableJson(v) {
  if (v === null || typeof v !== "object") return JSON.stringify(v);
  if (Array.isArray(v)) return "[" + v.map(stableJson).join(",") + "]";
  return (
    "{" +
    Object.keys(v)
      .sort()
      .map((k) => JSON.stringify(k) + ":" + stableJson(v[k]))
      .join(",") +
    "}"
  );
}

export function errText(e) {
  if (e == null) return t("common.unknownError");
  if (typeof e === "string") return e;
  if (e.message) return String(e.message);
  return String(e);
}

/** 一条联动条件是否成立；`also` 是第二层 AND，`equals` 命中任一即可。 */
export function showIfMatch(cond, getVal) {
  if (!cond || !cond.key) return true;
  const list = Array.isArray(cond.equals) ? cond.equals : [cond.equals];
  const cur = String(getVal(cond.key));
  if (!list.some((e) => String(e) === String(cur))) return false;
  return cond.also ? showIfMatch(cond.also, getVal) : true;
}

export function itemShowIf(fld, item) {
  if (!fld.show_if) return true;
  const list = Array.isArray(fld.show_if.equals) ? fld.show_if.equals : [fld.show_if.equals];
  return list.some((e) => String(e) === String(item[fld.show_if.key] ?? ""));
}

export function fmtBytes(n) {
  n = Number(n) || 0;
  if (!n) return "";
  if (n < 1024) return new Intl.NumberFormat(locale.value).format(n) + " B";
  const u = ["KB", "MB", "GB", "TB"];
  let i = -1;
  do {
    n /= 1024;
    i++;
  } while (n >= 1024 && i < u.length - 1);
  const value = Number(n.toFixed(1));
  return new Intl.NumberFormat(locale.value, { maximumFractionDigits: 1 }).format(value) + " " + u[i];
}

/** 「翻译模式」动态候选项；值写法与 active_engine 一致。
 *  这里是 ADR-015 零分支红线的有意例外之一（另外两处：删除后的顺位、
 *  弹窗认 name/vendor/model），三处都与 legacy 设置页保持同一口径。 */
export function engineChoices(getVal) {
  const listOf = (k) => {
    const v = getVal(k);
    return Array.isArray(v) ? v : [];
  };
  const out = [["microsoft", t("settings.engineMicrosoft")]];
  for (const p of listOf("vendor_profiles")) {
    if (p.enabled === false) continue;
    const name = String(p.name || "").trim() || t("settings.engineUnnamed");
    out.push(["vendor:" + p.id, p.use_llm ? name + t("settings.engineMachine") : name]);
    if (p.use_llm) out.push(["vendor:" + p.id + ":llm", name + t("settings.engineLlm")]);
  }
  for (const p of listOf("openai_profiles")) {
    if (p.enabled === false) continue;
    const name = String(p.name || "").trim() || t("settings.engineUnnamed");
    out.push(["openai:" + p.id, t("settings.engineOpenai") + name]);
  }
  return out;
}

/** 一条列表卡片的摘要：select 显示选项文字、switch 显示中文，其余原样。 */
export function summaryParts(f, item) {
  const keys =
    f.summary && f.summary.length
      ? f.summary
      : (f.item_fields || []).slice(0, 2).map((x) => x.key);
  const parts = [];
  for (const k of keys) {
    const fld = (f.item_fields || []).find((x) => x.key === k);
    let v = item[k];
    if (fld && fld.type === "select") {
      const hit = (fld.options || []).find((o) => String(o[0]) === String(v));
      if (hit) v = hit[1];
    } else if (fld && fld.type === "switch") {
      // use_llm 的「机翻 / 大模型」也是上述有意例外：只在摘要文案里用。
      v =
        k === "use_llm"
          ? v
            ? t("settings.listLlm")
            : t("settings.listMachine")
          : v
            ? t("settings.listOn")
            : t("settings.listOff");
    }
    if (v != null && String(v).trim() !== "") parts.push(String(v));
  }
  return parts;
}
