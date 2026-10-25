# dict-popup

离线划词词典的源代码。完整的安装、使用与构建说明见**仓库根目录的
[README](../README.md)**。

本目录是应用层 crate：

| 模块 | 职责 |
|---|---|
| `main.rs` | 启动顺序（先起 Win32 host，再起 eframe）、单实例、panic 处理 |
| `app.rs` | 共享状态、事件泵、查询编排、视口可见性 |
| `config.rs` | 配置读写（原子保存、坏 JSON 不覆写、容忍 BOM） |
| `dictionaries.rs` | 词典注册表、方向筛选、多词典分组查询 |
| `ui/card.rs` | 悬浮卡片（无边框、半透明、置顶、光标定位、淡入） |
| `ui/settings.rs` | 设置窗口（热键、词典、外观、行为、关于） |
| `platform.rs` | 光标/显示器/DPI 查询、打开目录、消息框 |
| `theme.rs` | 中文字体加载与配色 |
| `logging.rs` | 追加式日志（不记录选中文字） |

依赖：`dict-core`（StarDict 读取）与 `dict-input`（热键 / 托盘 / 取词）。

```bat
cargo build --release
cargo test --release
```
