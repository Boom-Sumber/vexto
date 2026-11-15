//! 关于：只读信息 + 几个动作按钮。没有要保存的字段，但仍是一个正常的 Tool
//! （这样它就和别的工具一样出现在左列表里，不用在设置页里写特例）。

use super::{Field, Section, Tool};

pub struct About;

impl Tool for About {
    fn id(&self) -> &'static str {
        "about"
    }

    fn name(&self) -> &'static str {
        "关于"
    }

    fn desc(&self) -> &'static str {
        "版本与许可"
    }

    fn icon(&self) -> &'static str {
        "info"
    }

    fn enabled(&self) -> bool {
        true
    }

    fn set_enabled(&self, _on: bool) -> Result<(), String> {
        Err("「关于」不能停用".to_string())
    }

    /// 只读信息页，没有「启用」这件事。
    fn switchable(&self) -> bool {
        false
    }

    fn status_line(&self) -> String {
        format!("Vexto {}", env!("CARGO_PKG_VERSION"))
    }

    fn sections(&self) -> Vec<Section> {
        vec![Section::new(
            "about",
            "关于",
            vec![
                Field::note("Vexto · 一个常驻托盘的轻量工具箱"),
                Field::note("划词翻译 · 截图 · 下载 · 局域网互传"),
                Field::note("本程序以 GNU GPL-3.0 协议开源。"),
                Field::note(
                    "本项目在功能与设计上借鉴了 Pot、ShareX、Rayburst、LocalSend 等开源项目，\
                     未拷贝其源代码；相关版权与商标归各自作者所有。",
                ),
                Field::note("本程序按「现状」提供，不对使用后果作任何担保。"),
            ],
        )]
    }
}
