// 字符错误率（CER，忽略空白）对比。
const rows = [
  {
    name: "大图 20px · Windows OCR",
    truth: `中文 OCR 测试：
需求 1：选框边框颜色跟随主色调（I/O）。
需求 2：放大框显示坐标与十六进制色值。
PaddleOCR 与 Windows OCR 对比：快捷、准确、离线。
Line I | l1 O0 ！ 《》 1234567890
看不清的字母 I 会不会被识别成《 符号？`,
    ocr: `中 文 OCR 测 试 ： 需 求 1 ： 选 框 边 框 颜 色 跟 随 主 色 调 (I/O ） 。 需 求 2 ： 放 大 框 显 示 坐 标 与 十 六 进 制 色 值 。 准 确 、 离 线 。 PaddleOCR 与 Windows OCR 对 比 ： 快 捷 Line1111 OO ！ 《 》 1234567890 看 不 清 的 字 母 丨 会 不 会 被 识 别 成 《 符 号 ？`,
  },
  {
    name: "小图 12px · Windows OCR",
    truth: `小字号中文测试（12px）：设置 → 截图 → 选框边框宽度。
字母 I 与数字 1、管道符 |、书名号《》容易混淆。
I | l1 O0 ！ 《》 1234567890 I/O
滚动截图、钉屏、放大镜、十六进制色值 #9EBED7。
看不清的字母 I 会不会被识别成《 符号？`,
    ocr: `小 字 号 中 文 测 试 〔 12px 〕 设 置 一 截 一 框 边 框 。 字 母 ] 与 数 字 1 、 管 道 符 书 名 号 《 》 容 易 混 淆 。 ] 卩 1 C 〕 0 ！ 《 》 1234567890 1/O 滚 动 截 条 、 钉 屏 放 大 憤 十 六 进 制 色 值 #9EBED70 看 不 渭 的 字 母 ] 会 不 会 被 识 引 成 《 符 号`,
  },
  {
    name: "大图 20px · PaddleOCR",
    truth: `中文 OCR 测试：
需求 1：选框边框颜色跟随主色调（I/O）。
需求 2：放大框显示坐标与十六进制色值。
PaddleOCR 与 Windows OCR 对比：快捷、准确、离线。
Line I | l1 O0 ！ 《》 1234567890
看不清的字母 I 会不会被识别成《 符号？`,
    ocr: `中文 OCR 测试:
需求1：选框边框颜色跟随主色调（I/O）。
需求2：放大框显示坐标与十六进制色值
PaddleOcR 与 Windows OCR 对比:快捷、准确、离线。
Line I  l1 O0 !
《》 1234567890
看不清的字母！会不会被识别成《符号？`,
  },
  {
    name: "小图 12px · PaddleOCR",
    truth: `小字号中文测试（12px）：设置 → 截图 → 选框边框宽度。
字母 I 与数字 1、管道符 |、书名号《》容易混淆。
I | l1 O0 ！ 《》 1234567890 I/O
滚动截图、钉屏、放大镜、十六进制色值 #9EBED7。
看不清的字母 I 会不会被识别成《 符号？`,
    ocr: `小字号中文测试（12px）：设置→截图→选框边框宽度。
字母1与数字1、管道符1、书名号《》容易混淆。
I11 00！《>1234567890 1/O
滚动截图、钉屏、放大镜、十六进制色值#9EBED7。
看不清的字母1会不会被识别成《符号？`,
  },
];

const norm = (s) => s.replace(/\s+/g, "");
function lev(a, b) {
  const m = a.length, n = b.length;
  let prev = Array.from({ length: n + 1 }, (_, j) => j);
  for (let i = 1; i <= m; i++) {
    const cur = [i];
    for (let j = 1; j <= n; j++) {
      cur[j] = Math.min(
        prev[j] + 1,
        cur[j - 1] + 1,
        prev[j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1)
      );
    }
    prev = cur;
  }
  return prev[n];
}

for (const r of rows) {
  const t = norm(r.truth);
  const o = norm(r.ocr);
  const d = lev(t, o);
  console.log(
    `${r.name}: CER=${((d / t.length) * 100).toFixed(1)}%  (真值 ${t.length} 字，编辑距离 ${d})`
  );
}
