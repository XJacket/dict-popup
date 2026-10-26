# dict-popup 设计契约

> 这份文件是并行开发的**接口契约**。三个工作流（输入层 / 界面层 / 发行工程）都按它实现，
> 不要私自改接口；需要改先改这里。

## 1. 产品定义

`dict-popup` 是一个 Windows 桌面小工具：**按自定义热键 → 读取当前选中的文字 → 查本地 StarDict 词典 → 在鼠标旁弹出悬浮卡片**。
完全离线，不启动 GoldenDict，单 exe、无运行时依赖。

取词策略（0.1.x 修订）：**优先模拟 `Ctrl+C` + 读剪贴板**，读完立即还原原来的剪贴板文本；
UI Automation 作为第二条路径。原因：UIA 在 XAML/Electron/自绘程序里普遍不可靠。

**取代** `legacy/goldendict-uia-hotkey`，但不删除它。

## 2. 验收标准（发行门槛）

| 项 | 门槛 |
|---|---|
| 内存 | 空闲工作集 ≤ 50 MB；卡片打开峰值 ≤ 105 MB；私有内存 ≤ 70 MB |
| 热键到卡片可见 | ≤ 250 ms（本机 ECDICT，不含首次冷启动） |
| 查询延迟 | ≤ 5 ms/词典（dict-core 实测 5–12 µs，含 UI 渲染另计） |
| 冷启动到托盘就绪 | ≤ 1.5 s |
| exe | release 单文件，≤ 15 MB，无外部 DLL |
| 崩溃 | 无 panic 路径；所有 Win32 调用检查返回值 |
| 隐私 | **日志绝不记录选中文字内容**，只记录长度 |
| 单实例 | 第二次启动只唤起设置窗口，不产生第二个托盘图标 |
| 渲染后端 | **glow（OpenGL）**，禁止 wgpu |

> 实测（2026-10-24，空窗口基线）：eframe 0.37 默认的 wgpu 后端 **WS 236 MB / 私有 469 MB / 52 线程**，超预算 2.4 倍；
> 改用 glow 后 **WS 90 MB / 私有 59 MB / 13 线程**。因此依赖必须写死：
>
> ```toml
> eframe = { version = "0.37", default-features = false, features = ["glow", "default_fonts", "clipboard", "links"] }
> ```
>
> 附带要求：动画必须结束（150 ms 淡入 OK），空闲时不得持续 60 fps 重绘；字体只加载一个中文字族，
> 字号只用实际用到的几个档位。
>
> **最终实测**（`dist\dict-popup.exe`，2026-10-24）：私有内存 55–64 MB；卡片打开时工作集峰值约 101 MB；
> 卡片关闭后归还工作集，空闲 WS 3–14 MB。`scripts\measure-memory.ps1 -Launch` 20 秒采样
> min 2.1 / avg 14.0 / max 95.0 MB，门禁 PASS。三项关键优化：glow 代替 wgpu、索引改流式扫描
> （工作集 97 MB → 21 MB）、每 32 条一个锚点的稀疏索引（表 13.6 MB → 0.4 MB）。

## 3. 架构

```
┌─────────────────────────┐   Event (mpsc)   ┌──────────────────────┐
│ dict-input (Win32 线程)  │ ───────────────► │ dict-popup (egui 主线程) │
│  · 消息循环 / 热键        │ ◄─────────────── │  · 卡片窗口 / 设置窗口    │
│  · 托盘 / 单实例          │   Command (mpsc) │  · 配置读写              │
│  · anchor 采样           │                  │  · request 状态机        │
│  · capture 流水线         │                  │  · 词典查询编排          │
└─────────────────────────┘                  └──────────┬───────────┘
                                                        │ 调用
                                              ┌─────────▼──────────┐
                                              │ dict-core (纯逻辑)  │
                                              │  · StarDict 读取     │
                                              │  · mmap + 二分       │
                                              │  · 方向指纹          │
                                              └────────────────────┘
```

**dict-input 内部模块**（`capture` 是唯一拥有取词策略的地方）：

| 模块 | 职责 |
|---|---|
| `host.rs` / `host_impl.rs` | 消息循环、热键、托盘、单实例；`Host` 在创建线程处构造 |
| `anchor.rs` | 热键触发瞬间采样光标位置、工作区、显示器缩放 |
| `capture/mod.rs` | `capture(target) -> Result<CapturedText, CaptureFailure>`，唯一的取词入口 |
| `capture/pipeline.rs` | 有序策略、超时预算、剪贴板还原；可注入 `CaptureEnv` |
| `capture/win32.rs` | `CaptureEnv` 的 Win32 适配器（Pot crate、Ctrl+C、WM_COPY、剪贴板） |
| `capture/uia.rs` | UI Automation 搜索（最后一条策略） |
| `capture/text.rs` | `CapturedText`：唯一构造函数，trim/去空/限长 |
| `capture/service.rs` | 取词 worker/coordinator 线程 |
| `diagnostics.rs` | 结构化诊断报告；通过装饰真实 `CaptureEnv` 观察同一条流水线 |

**线程规则**
- 主线程：eframe 事件循环，**不得**直接调用 Win32 或阻塞 IO。
- Host 线程：消息循环、热键、托盘；不碰 UI。热键到达时先采样 `Anchor`，再发起取词。
- Capture 线程：跑完整策略流水线（Pot crate → Ctrl+C → WM_COPY → UIA），`COINIT_MULTITHREADED`；
  预算 `CAPTURE_BUDGET`（1.5 s）。超时只丢弃本次并回报 `CaptureFailure::TimedOut`，
  不得杀死线程；并发请求返回 `CaptureFailure::Busy`（「忙则跳过」）。
- 事件到达 UI 后必须 `ctx.request_repaint()`。

**事件契约**（`dict_input::Event`）：`Hotkey { anchor }` / `Selection(CapturedText)` / `SelectionFailed(CaptureFailure)` / `HotkeyFailed(String)` / `Tray(TrayAction)` / `SecondLaunch`。
- `Hotkey` 携带按键时刻的锚点，UI 不再自己查光标/显示器/DPI。
- `Selection` / `SelectionFailed` 只用 `capture` 的类型：文本已校验，失败是枚举而非字符串。
- `HotkeyFailed` 携带完整中文提示；**注册失败时旧热键继续有效**，UI 必须把它显示给用户，不能只写日志。
- `Host::start` 必须在 `eframe::run_native` **之前**调用（内部要设 Per-Monitor-V2 DPI awareness，winit 建窗后就改不动了）。

## 4. 配置 schema（`%APPDATA%\dict-popup\config.json`）

```jsonc
{
  "version": 1,
  "hotkey": "Ctrl+Alt+D",              // 规范化字符串，见 dict-input::hotkey
  "dictionaries": [
    {
      "path": "D:\\...\\stardict-ecdict-2.4.2.ifo",
      "label": "ECDICT 简明英汉",          // 用户可改，默认取 .ifo 的 bookname
      "enabled": true,
      "direction": "auto"                // "auto" | "en-zh" | "zh-en" | "en-en" | "zh-zh"
    }
  ],
  "ui": {
    "theme": "system",                   // "system" | "light" | "dark"
    "font_size": 15.0,
    "card_width": 460.0,
    "max_card_height": 520.0,
    "close_on_focus_lost": true,
    "auto_hide_seconds": 0,              // 0 = 不自动隐藏
    "expand_first_dictionary": true
  },
  "behavior": {
    "trim_selection": true,
    "autostart": false
  }
}
```

规则：
- 缺失文件 → 写默认值（**仅在文件不存在时**；解析失败绝不覆写，见设计约束 9）。
- 字段缺失 → 用默认值补齐（`#[serde(default)]`）。
- `direction = "auto"` 时用 `dict_core::fingerprint()` 的结果，置信度 < 0.85 时在设置里标黄提示"自动识别不确定"。
- 保存用「临时文件 + 原子替换」，保留 `config.json.bak`。

## 5. 卡片规格

- 无边框、置顶、不进任务栏、`WS_EX_TOOLWINDOW`；圆角 12 px + 1 px 描边 + 150 ms 淡入。
  窗口尺寸**等于**卡片尺寸，四周不留透明内边距——投影在透明窗口里会被窗口边缘裁出一道生硬
  的矩形边（看起来像第二层边框），而且那圈透明区域会吃掉本该落到下层窗口的点击。
- 宽度取配置；高度自适应，超过 `max_card_height` 内部滚动。
- 结构：
  - 头部：词条（大字）+ 音标 + 方向徽章（英→中）+ 来源词典名。
  - 正文：按词典分组；**第一组展开，其余折叠**并显示命中条数；组内是释义。
  - 底部：提示条 `Esc 关闭 · 再按热键收起`。
- 文字**可选中复制**。
- 位置：鼠标右下方偏移 (16, 20)；碰到屏幕边缘自动翻转；按所在显示器的 DPI 计算，禁止跨屏错位。
- 关闭：Esc / 失焦（可配置）/ 再次按热键 / 点击卡片外。
- 卡片只能有一个；重复触发热键时**替换内容**而不是叠开新窗口。

## 6. 设置窗口

单窗口、分节：

1. **热键**：捕获控件（按下组合键即记录），非法组合给出原因；提供"恢复默认"。
2. **词典**：列表（拖动排序 / 启用勾选 / 删除 / 添加 .ifo 文件）；每行显示自动识别的方向、置信度、词条数；错误（文件丢失、格式不支持）标红并可移除。
3. **外观**：主题、字号、卡片宽度。
4. **行为**：开机自启、去首尾空白、失焦关闭、自动隐藏秒数。
5. **关于**：版本、日志路径（可打开目录）、配置路径、许可证。

保存即生效（热键重新注册），不做"需要重启"。

## 7. 取词与查询流程

1. 热键 → host 采样 `Anchor`（光标、工作区、缩放）→ `Event::Hotkey { anchor }`。
2. UI 开一个 `request`：`Running { deadline }`，不显示"查询中"占位，等结果或超时。
3. Host 调 `capture::capture(target)`，内部按固定顺序尝试：
   1. **Pot crate**（`selection`，与 Pot 同一实现，含修饰键复位与图片还原）
   2. **Ctrl+C 剪贴板**（先显式松开 9 个修饰键，再发干净的 Ctrl+C，轮询剪贴板序号）
   3. **WM_COPY**（直接让前台焦点控件自己复制，不依赖合成输入）
   4. **UI Automation**（`GetFocusedElement` → `TextPattern` → `GetSelection`；无 TextPattern 时回退 `ValuePattern`）
   顺序固定在本模块内，调用方无法调整；每次尝试都有独立预算，总预算 1.5 s。
4. 取到文本 → `CapturedText::new(raw, trim)`：按 `trim_selection` 处理、拒绝空白、按字符边界限长。
   失败的语义在 `CaptureFailure` 里（`NoSelection` / `NoTextPattern` / `InputBlocked` / `ClipboardBusy` / `TimedOut` / `Busy` / `Platform`），
   文案只在它的 `Display` 里生成一次。
5. 剪贴板还原：取词前快照一次，取词结束后**恰好写回一次**（成功或失败）。
6. `CapturedText` → `dict_core::normalize::candidates()` → 逐个候选在**方向匹配**的词典里查；命中即停。
7. 方向匹配：查询文字含汉字 → 只查 `zh-en` / `zh-zh`；纯 ASCII → 只查 `en-zh` / `en-en`；`auto` 方向按指纹结果归组。
8. 全部未命中 → 卡片显示"未收录：<词>"，并列出试过的候选（最多 3 个）。
9. 取词失败 → 卡片提示（`CaptureFailure` 的中文文案），不弹模态框。
   关闭卡片（Esc / 失焦 / 再按热键）会 `dismiss()` 请求：迟到的结果按状态机丢弃，**不会重新弹卡**。

## 8. 发行清单

- `build.bat`：设 PATH（MinGW + cargo），`cargo build --release`，产出 `dist\dict-popup.exe`。
- 版本资源（`winres`/`windres`）：`FileDescription`/`ProductName`/`FileVersion` = `0.1.0`。
- 图标 `assets/dict-popup.ico`（多尺寸 16/32/48/256），托盘与 exe 共用。
- `dist\` 内含：exe、`README.md`、`LICENSE`、`CHANGELOG.md`、`config.example.json`。
- `scripts\package.ps1`：构建 + 复制 + 打 zip（`dict-popup-0.1.0-win64.zip`）。
- README：安装、首次配置（选词典）、热键、限制、常见问题。

## 9. 设计约束（硬性）

1. **允许模拟 `Ctrl+C` 取词，但必须**：取词前备份剪贴板文本、取词后还原；不得发送其它合成按键；取词失败要告知用户而不是静默。
2. **日志不写选中文字**，只写长度与命中词典名。
3. 配置解析失败**不得**覆写用户文件；报错并让用户选择。
4. 所有 `unsafe` 块必须有 `// SAFETY:` 注释说明前置条件。
5. 生产路径禁止 `unwrap`/`expect`（`main` 启动失败除外，且必须弹窗告知）。
6. UI 文案全部中文，错误必须对用户可见（托盘气泡/卡片/设置页），不能只写日志。
7. 所有 Win32 调用检查返回值；托盘 `Shell_NotifyIconW` 失败要重试。
8. 单实例：命名互斥体 + 第二实例通过 `WM_COPYDATA` 或注册消息唤起设置。
9. 热键重注册流程：先解析新值 → 成功才释放旧的；失败保留旧热键并回报错误。

## 10. 测试策略

- `dict-core`：16 个单元测试（解析、回退链、方向指纹）。
- `dict-input`：59 个。热键模型与 `HotkeyError` 文案、`CapturedText` 不变量、`CaptureFailure` 文案、
  capture pipeline（注入 `CaptureEnv` 假实现：策略顺序、还原恰好一次、未变化不算复制、超时、冻结时钟不死循环）、
  diagnostics 报告渲染、托盘/消息/autostart 纯逻辑。
- `dict-popup`：56 个。配置读写、词典注册表、方向筛选、候选排序、request 状态机
  （idle→running→done、dismiss 丢弃迟到结果、busy 与 timed-out 不同、看门狗由预算派生）、锚点定位。
- 与真实程序交互的取词（剪贴板/UIA 在 Chrome、记事本、Office 里的行为）只能手工验证，记录在 QA 清单。
- 手工 QA 清单（`docs/QA.md`）：Chrome/记事本/Office/PDF 取词；双屏不同缩放；热键冲突；无选中文字；词典文件被移动；Explorer 重启后托盘图标恢复。

## 11. 目录归属（并行开发边界）

| 目录/文件 | 负责人 |
|---|---|
| `dict-core/**` | 已冻结，只有 orchestrator 可改 |
| `dict-input/**` | 输入层 agent |
| `dict-popup/**` | 界面层 agent |
| `assets/**`、`scripts/**`、`build.bat`、`dist/**`、`README.md`、`CHANGELOG.md`、`LICENSE` | 发行工程 agent |
| `docs/DESIGN.md`、`docs/QA.md` 的前 11 节 | orchestrator（QA 清单由界面层补充） |

同一文件不要两个人同时改；发现接口需要变更，先在回复里说明，由 orchestrator 统一改契约。
