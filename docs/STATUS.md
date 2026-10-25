# 交付状态（0.1.0）

本文件记录**已验证**与**仍待人工验收**的项目，避免把"能编译"当成"能用"。

## 已验证（自动化 / 可复现命令）

| 项目 | 结果 | 复现方式 |
|---|---|---|
| 三个 crate 构建 | 全部 `cargo build --release` 通过，无警告 | 各目录 `cargo build --release` |
| 单元测试 | **dict-core 16 + dict-input 31 + dict-popup 44 = 91 个全过** | 各目录 `cargo test --release` |
| 真实词典查询 | ECDICT 3,402,564 条，查询 5–12 µs，扫描 79 ms | `dict-core → cargo run --release --bin probe -- <ifo>` |
| 方向指纹 | 英→中，置信度 0.98（词头汉字率 0.000 / 释义 0.984） | 同上 |
| 单实例（跨进程） | 第二个进程返回 `AlreadyRunning`，第一个收到 `SecondLaunch` | `dict-input → cargo run --release --example smoke` |
| 启动耗时 | Host 就绪 11 ms | 同上 |
| 内存 | 私有 55–64 MB；卡片打开 WS 峰值 ~101 MB；空闲 WS 3–14 MB | `scripts\measure-memory.ps1 -Launch`（20 s，PASS） |
| 资源注入 | `dist\dict-popup.exe` FileVersion 0.1.0.0，四档图标（16/32/48/256） | `build.bat` + `(Get-Item dist\dict-popup.exe).VersionInfo` |
| 打包 | `dist\dict-popup-0.1.0-win64.zip`（3.1 MB） | `scripts\package.ps1` |
| 卡片渲染 | 截图确认：词条、音标、方向徽章、分组、释义、页脚提示均正常 | `dict-popup.exe --demo apple` |
| 配置健壮性 | 坏 JSON 不覆写、字段缺失补默认、**容忍 UTF-8 BOM** | `dict-popup → cargo test release`（config 模块 8 个测试） |

## 仍待人工验收（需要真人在桌面上操作，见 `docs/QA.md`）

1. **UIA 取词**：Chrome / 记事本 / Word / PDF / VS Code 中选中文字后按热键。
2. **托盘**：图标显示、菜单三项、Explorer 重启后自动恢复、注销时消失。
3. **热键冲突**：被占用时报错且旧热键仍可用。
4. **双屏混合缩放**：卡片在光标所在屏正确弹出、边缘翻转、不跨屏错位。
5. **开机自启**：勾选后重启验证；移动 exe 后复选框应显示未启用。
6. **冷启动 ≤ 1.5 s**：从双击到托盘可用（脚本只测了进程内存）。

> 自动化注入按键在本机不可行：桌面正被使用，`SendKeys` 的 Alt 组合会落进别的窗口
> （实测跑到 Lightroom 里开了右键菜单）。因此交互类验收必须人工执行，`scripts\e2e-smoke.ps1`
> 与 `--demo <词>` 只覆盖"查询 → 渲染"这一段。

## 已修复缺陷（0.1.0 发布前）

### 取词在各类程序中全部失败（2026-10-25）

**结论**：UIA 路线在 Windows 11 记事本（XAML）、Electron、自绘程序里不可靠——顶层窗口没有
文本模式，真正的控件藏在子树里，而部分提供程序连 `FindAll(Descendants)` 都不返回内容。

**改为 Pot 的做法**：热键后
1. 备份剪贴板文本；
2. **等 Ctrl/Alt/Shift/Win 松开**（热键是 Ctrl+Alt+D，不等待的话目标程序看到的是 `Ctrl+Alt+C`）；
3. 合成 `Ctrl+C`，轮询剪贴板序列号，最长 450 ms；
4. 读取 `CF_UNICODETEXT`（缺失时回退 `CF_TEXT`），**立即还原原剪贴板文本**；
5. 注入没到达（剪贴板序号不变）时才回退到 `WM_COPY`，最后才回退到 UIA。

顺序固定在 `capture/pipeline.rs`，不再有环境变量开关（`DICT_POPUP_CAPTURE_MODE` 已删除）。
剪贴板读写的往返已由 `dict-input/examples/clipboard_probe.rs` 实测通过。

### 热键按下后卡片不出现（2026-10-24）

**症状**：按热键听到提示音，屏幕上看不到卡片。

**两个独立原因**：
1. **旧工具占用热键**：`goldendict-uia-hotkey.exe` 仍在运行并持有 `Ctrl+Alt+D`，响应的是它（它取词失败时只响铃、不弹窗）。该程序不在启动项里，属于手动遗留进程。
2. **卡片视口可见性缺陷**：`ViewportBuilder::with_visible()` 只在视口**创建**时生效。卡片视口在启动时以「隐藏」创建，之后热键把它置为可见时没有任何 `ViewportCommand::Visible(true)` 发出，于是窗口存在、位置尺寸正确，但永远 `IsWindowVisible == false`。`--demo` 能看见是因为它在首次 `ui()` 之前就置了可见，正好赶上创建。

**修复**：`App::ui` 里显式跟踪并推送两个子视口（卡片 / 设置）的可见性变化
（`src/app.rs` 的 `card_visible_sent` / `settings_visible_sent`）。

**回归验证方式**（无需人工按键）：向宿主窗口投递 `WM_HOTKEY`，然后断言卡片窗口可见：

```powershell
$host = 找到类名 'DictPopupInputHost' 的窗口
PostMessage($host, 0x0312, 热键id, 0)
# 期望：出现一个 class='Window Class'、标题 'dict-popup'、500x202、IsWindowVisible=True 的窗口
```

投递 `WM_KEYDOWN VK_ESCAPE` 到该窗口 → 应回到 `visible=False`。两条均已实测通过。

## 已知限制

- 仅支持 StarDict（`.ifo` + `.idx` + 未压缩 `.dict`）；`.dict.dz` 明确报"不支持压缩词典"。
- 无发音（本机无离线音频）、无模糊/前缀搜索、无文件选择对话框（手动填路径）。
- `Win` 键无法在捕获控件里录入（winit 只上报 Ctrl/Alt/Shift），需手动填写。
- 关闭"失焦关闭"后，点击卡片外不会收起（需要全局鼠标钩子，未做）。
- 普通 `cargo build` 出的 exe 不含图标/版本资源；发行必须走 `build.bat`。
- 托盘左侧单击也会弹出菜单（Windows 11 通知区 v4 行为），双击打开设置。

## 发布

```bat
build.bat
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\package.ps1
```

产物：`dist\dict-popup-0.1.0-win64.zip`（exe + README + LICENSE + CHANGELOG + 配置样例）。
