# Changelog

本项目的所有重要变更都记录在此文件。

格式参考 [Keep a Changelog 1.1.0](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本 2.0.0](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### 新增

- **UIAccess 支持**（`build.bat /uiaccess`）：manifest 声明 + 签名 + 安装到 `%ProgramFiles%`
  后，进程被提升到 High 完整性级别，可以跨过 UIPI 往提权窗口注入合成输入。
  默认构建仍为 `uiAccess="false"`（未签名时带 `true` 会**无法启动**）。
- `dict-input/src/privilege.rs`：报告 `uiaccess` / `elevated` / `integrity`，
  写启动日志、设置页状态行与 `diagnostics` 报告。
- `scripts/install-uiaaccess.ps1`、`scripts/setup-uiaaccess-local.ps1`、`docs/UIAACCESS.md`。
- `docs/adr/0001-uiaaccess-for-cross-integrity-capture.md`：记录为什么签名本身不够。

### 实测

自签证书 + `%ProgramFiles%` 安装、非提权启动：`uiaccess=true elevated=false integrity=High`，
并在以管理员身份运行的记事本里成功取词。

## [0.2.0] - 2026-10-25

第二个版本，**首个公开发布**。标记为测试版：功能可用，但取词路径尚未在各家应用里逐项验证。

### 变更（架构）

- **取词收成一个 module**（`dict-input/src/capture/`）：Pot crate → Ctrl+C → WM_COPY → UIA 的顺序、超时预算、
  剪贴板还原都归它所有；删除 `DICT_POPUP_CAPTURE_MODE`、`prefer_clipboard` 与两条重复的轮询循环。
  新增 `CaptureEnv` seam 与 9 个无需桌面的 pipeline 测试（策略顺序、失败还原一次、未变化不算复制、超时）。
- **`CapturedText`** 成为唯一可传给查询层的文本表示：构造时 trim、拒绝空白、按字符边界限长，
  下游不再重复判断（原先同一个判断散在 5 处）。
- **`CaptureFailure`** 取代 `Automation(String)` 兜底：`NoSelection` / `NoTextPattern` / `InputBlocked` /
  `ClipboardBusy` / `TimedOut` / `Busy` / `Platform`，中文文案只在 `Display` 里生成。
- **诊断独立成 module**（`dict-input/src/diagnostics.rs`）：用装饰器包住真实的 `CaptureEnv` 跑同一条流水线，
  输出结构化 `Report`；删除 `debug_capture_once` 那条约 340 行的并行实现与 7 个 `debug_*` 导出。
- **热键只有一种表示**：`Hotkey::new(modifiers, key)` 直接构造，`canonical()` 是唯一拼写；
  设置窗口不再"拼字符串再解析"，键名表与错误文案各只有一份。
- **一次取词只有一个状态机**（`dict-popup/src/request.rs`）：`Idle / Running{deadline} / Done / Failed / TimedOut`。
  修复 "Esc 关闭的卡片在词典加载完成后自己弹回"（`card.pending` 重放）。
- **`Event::Hotkey` 携带 `Anchor`**（光标、工作区、缩放，热键瞬间采样）；UI 不再自己查 Win32，
  DPI 只有锚点一个来源，卡片定位改为纯函数。
- 单一超时权威：`capture::CAPTURE_BUDGET`（1.5 s），UI 看门狗由它派生（+300 ms 投递宽限）。
- 删除 pass-through：`Host::from_parts`、`host::channel`、`HostError::HotkeyUnavailable`、
  未使用的 autostart 读取路径、`SearchStats`。

### 测试

- `dict-core` 16、`dict-input` 59、`dict-popup` 56（原 16 / 31 / 44）。

### 已知问题

- **未签名**：`0.2.0` 没有代码签名证书，Windows 会提示"未知发布者"。
- **普通权限下可能取不到词**：实测管理员身份运行正常、普通启动失败；成因指向进程完整性级别
  与系统对未签名程序的限制，签名后能否恢复待实机确认。
- 托盘图标在部分系统上不显示（未定位）。
- 取词只在 Windows 11 记事本里完成过实机验证，Chrome / Office / PDF 未逐项验证。
- 无发音、无模糊搜索、无文件选择对话框。

## [0.1.0] - 2026-10-24

首个发布版本：离线划词词典，取代 `legacy/goldendict-uia-hotkey` 的“热键 + GoldenDict”组合。

### 新增

- 全局热键取词：默认 `Ctrl+Alt+D`，可在设置中捕获修改；注册失败保留旧热键并给出提示。
- 通过 Windows UI Automation（`UIA_TextPattern`，缺失时回退 `UIA_ValuePattern`）读取当前选区，
  全程不读写剪贴板、不发送合成按键。
- 本地 StarDict 查询：`.ifo` / `.idx` / `.dict`(含 `.dict.dz`)，mmap + 二分查找，
  多词典按配置顺序查询、按方向筛选，命中即停。
- 方向自动识别（`auto` / `en-zh` / `zh-en` / `en-en` / `zh-zh`），置信度低于 0.85 时在设置中标黄。
- 悬浮卡片：无边框、置顶、不进任务栏、圆角阴影、150 ms 淡入，按显示器 DPI 定位并在屏幕边缘翻转；
  按词典分组、第一组展开其余折叠、文字可选中复制；`Esc` / 失焦 / 再按热键关闭。
- 设置窗口：热键、词典（排序/启用/删除/添加）、外观（主题/字号/卡片宽度）、
  行为（开机自启/去首尾空白/失焦关闭/自动隐藏）、关于（版本/日志/配置/许可证）。
- 托盘图标与菜单；单实例（命名互斥体，第二次启动唤起设置窗口，不产生第二个托盘图标）。
- 配置保存在 `%APPDATA%\dict-popup\config.json`：缺失字段补默认值，保存采用临时文件 + 原子替换并保留 `.bak`，
  解析失败绝不覆写用户文件。
- 日志只记录选中文字的长度与命中的词典名，不记录文字内容。

### 构建与发行

- 多尺寸图标 `assets/dict-popup.ico`（16/32/48/256，256 为 PNG 压缩帧），由 `scripts/make-icon.ps1` 生成。
- `assets/version.rc`：`ProductName` / `FileDescription` / `FileVersion` 0.1.0.0 等版本资源与图标，
  由 `windres` 编译后通过 `cargo rustc -- -C link-arg=...` 注入。
- `build.bat` 一键构建 `dist\dict-popup.exe`（图标 + 版本资源，缺失则构建失败）。
- `scripts/package.ps1` 生成 `dist\dict-popup-0.1.0-win64\` 与同名 zip。
- `scripts/measure-memory.ps1` 采样 Working Set，按 100 MB 门槛给出 PASS/FAIL。
- `docs/QA.md`：手工验收清单（取词矩阵、双屏缩放、热键冲突、托盘恢复、单实例、内存、冷启动等）。
