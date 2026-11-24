import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "frontend");
const zh = JSON.parse(fs.readFileSync(path.join(root, "i18n/locales/zh-CN.json"), "utf8"));
const en = JSON.parse(fs.readFileSync(path.join(root, "i18n/locales/en.json"), "utf8"));

function flatten(node, prefix = "", out = new Map()) {
  for (const [key, value] of Object.entries(node)) {
    const full = prefix ? `${prefix}.${key}` : key;
    if (value && typeof value === "object") flatten(value, full, out);
    else out.set(full, value);
  }
  return out;
}

const zhKeys = flatten(zh);
const enKeys = flatten(en);
const missingZh = [];
const missingEn = [];
const unused = new Set(zhKeys.keys());
const files = [];

function walk(dir) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full);
    else if (/\.(vue|js)$/.test(entry.name)) files.push(full);
  }
}
walk(root);

for (const file of files) {
  const text = fs.readFileSync(file, "utf8");
  const re = /(?:\$t|\bt)\(\s*["'`]([^"'`]+)["'`]/g;
  for (const match of text.matchAll(re)) {
    const key = match[1];
    if (!key.includes(".")) continue;
    unused.delete(key);
    if (!zhKeys.has(key) || typeof zhKeys.get(key) !== "string" || zhKeys.get(key) === "") {
      missingZh.push(`${path.relative(root, file)}: ${key}`);
    }
    if (!enKeys.has(key)) missingEn.push(`${path.relative(root, file)}: ${key}`);
  }
}

if (missingZh.length) {
  console.error("missing or empty zh-CN keys:\n" + missingZh.join("\n"));
}
if (missingEn.length) {
  console.error("missing en structure:\n" + missingEn.join("\n"));
}
if (missingZh.length || missingEn.length) process.exit(1);

console.log(`i18n ok: ${zhKeys.size} zh keys, ${enKeys.size} en keys, ${files.length} files scanned`);
if (unused.size) {
  console.log("unreferenced zh keys (may be dynamic or reserved):");
  for (const key of [...unused].sort()) console.log("  " + key);
}
