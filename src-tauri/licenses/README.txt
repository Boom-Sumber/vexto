Vexto 随包分发的第三方程序许可说明
=====================================

本目录只包含许可文本，不是程序源码。

aria2c（下载引擎 sidecar）
-------------------------
许可：GNU General Public License, Version 2（GPLv2）
许可全文：aria2-COPYING
版本与二进制校验值：aria2-VERSION.txt

源码获取方式（GPLv2 §3 要求）：
aria2 是独立可执行文件，Vexto 只通过 127.0.0.1 的 JSON-RPC 与它通信，
使用的是官方 release 的未修改原版二进制。对应版本的完整源码可从项目官方仓库获取：

  https://github.com/aria2/aria2
  对应 tag：release-1.37.0

如需书面要约或在三年内索取该版本源码，请从上述仓库按 tag 下载；
本程序不分发 aria2 的修改版，也不把它的代码合并进 Vexto 主程序。

Vexto 主程序的依赖均为 MIT / Apache-2.0 等宽松许可，
未引入任何 GPL/AGPL 的 Rust crate（见 docs/07-LICENSING.md §7.5）。
