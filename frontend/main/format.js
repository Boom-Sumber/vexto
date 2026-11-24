import { locale } from "../i18n/index.js";

export function fmtBytes(n) {
  n = Number(n) || 0;
  if (n < 1024) return new Intl.NumberFormat(locale.value).format(n) + " B";
  const units = ["KB", "MB", "GB", "TB"];
  let i = -1;
  do {
    n /= 1024;
    i++;
  } while (n >= 1024 && i < units.length - 1);
  const value = Number(n.toFixed(n < 10 ? 1 : 0));
  return new Intl.NumberFormat(locale.value, { maximumFractionDigits: 1 }).format(value) + " " + units[i];
}

export function fmtSpeed(n) {
  return fmtBytes(n) + "/s";
}

export function fmtCount(n) {
  n = Number(n) || 0;
  return new Intl.NumberFormat(locale.value, {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(n);
}

export function fmtEta(seconds, t) {
  if (seconds == null) return "—";
  const s = Number(seconds) || 0;
  if (s < 60) return t("main.etaSeconds", { n: s });
  if (s < 3600) return t("main.etaMinutes", { n: Math.ceil(s / 60) });
  return t("main.etaHours", { n: (s / 3600).toFixed(1) });
}
