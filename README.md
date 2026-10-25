# dict-popup

> **测试版（0.2.x）** —— 功能可用但仍在验证中，接口与行为可能调整。
> 首个公开版本为 `0.2.0`，未签名；签名与 1.0 计划见 [代码签名](#代码签名)。

**离线划词词典，Windows 桌面小工具。** 选中任意程序里的文字，按一次热键，鼠标旁弹出释义卡片。
不联网、不启动 GoldenDict、单文件 exe、零运行时依赖。

> Offline word-lookup popup for Windows. Select text anywhere, press a hotkey, get a
> floating card with the definition. No network, no runtime, one exe.

## 为什么是它

- **完全离线**：直接读本地 [StarDict](https://en.wikipedia.org/wiki/StarDict) 词典（如 ECDICT 340 万词条），
  不联网、不上传任何内容。
- **轻量**：release 单 exe 约 6.5 MB，私有内存 55–65 MB，空闲工作集 3–14 MB
  （实测数据见 [docs/STATUS.md](docs/STATUS.md)）。
- **快**：340 万词条的词典，单次查询 5–12 µs；打开词典（含建索引）约 80 ms。
- **悬浮卡片**：无边框、半透明、置顶、跟随鼠标出现，不会打断你正在做的事。

## 安装

1. 从 [Releases](../../releases) 下载 `dict-popup-0.2.0-win64.zip`（也可以从源码构建，见下节）
2. 解压到**任意普通目录**（例如 `C:\Users\<you>\Apps\dict-popup`）
   - ⚠️ 不要把 exe 放在被系统标记为「低完整性」的目录里（某些沙箱/受限目录），
     否则 Windows 会阻止它向其他程序发送按键，取词会失败
3. 运行 `dict-popup.exe`，首次启动会打开设置窗口

> **未签名**：`0.2.0` 没有代码签名证书，Windows 会提示"未知发布者"或"Windows 已保护你的电脑"。
> 这是未签名开源软件的预期行为，可点「更多信息 → 仍要运行」。
> 若在**普通权限**下取不到词，见下方[已知限制](#已知限制)。

## 首次配置

设置 → 词典 → 添加 `.ifo` 文件，指向你的 StarDict 词典，例如：

```
D:\dicts\ecdict\stardict-ecdict-2.4.2.ifo
```

需要 `.ifo` + `.idx` + 未压缩的 `.dict` 三个文件，放在同一目录。
词典可以从 [ECDICT](https://github.com/skywind3000/ECDICT) 转换而来。

## 使用

1. 在浏览器 / 记事本 / Office 等程序里**用鼠标选中**一个英文单词或短语
2. 按 **Ctrl+Alt+D**（可在设置里改）
3. 鼠标右下方弹出卡片：词条、音标、方向徽章、按词典分组的释义
4. **Esc** 关闭，或再按一次热键收起

> 取词方式：模拟一次 `Ctrl+C` 读取选中文字，随后**立即还原你原来的剪贴板内容**（文本与图片）。
> 这是目前在各类程序里最可靠的方式（Pot 等项目采用同样方案）。代价是这一瞬间剪贴板被占用。

## 从源码构建

需要 Rust（GNU 工具链）与 MinGW-w64：

```bat
build.bat
```

产物在 `dist\dict-popup.exe`（含图标与版本资源）。
打包：`powershell -NoProfile -ExecutionPolicy Bypass -File scripts\package.ps1`

## 架构

```
dict-core/   StarDict 读取（mmap + 稀疏锚点索引）、方向指纹、查询回退链
dict-input/  全局热键、托盘、单实例、取词（剪贴板 / UI Automation）
dict-popup/  egui 界面：悬浮卡片、设置窗口、配置与查询编排
```

细节见 [docs/DESIGN.md](docs/DESIGN.md)（接口契约与验收标准）。

## 已知限制

**功能边界**

- 仅支持 StarDict（`.dict.dz` 压缩格式暂不支持）
- 无发音（本机无离线音频）、无模糊/前缀搜索
- 无文件选择对话框，词典路径需手动粘贴

**验证状态（0.2.0，如实说明）**

- 取词只在 **Windows 11 记事本**里完成过实机验证（`Ctrl+C` 路径）；
  Chrome / Office / PDF / Electron 应用**尚未逐项验证**
- 托盘图标在部分系统上不显示，尚未定位（[issue](.scratch/dict-popup/issues/01-tray-icon-missing.md)）
- **以普通权限运行时可能取不到词**：实测以管理员身份运行一切正常，普通启动失败。
  成因指向进程完整性级别与 Windows 对未签名程序的限制，**尚未被签名验证排除**；
  签名后是否恢复普通权限可用，需要一次实机确认

**环境要求**

- 不要放在被标记为「低完整性」的目录（沙箱、部分受限目录），否则系统会丢弃合成输入

## 代码签名

本项目的 release 目前**未用公共 CA 签名**。如果你从源码自行构建，Windows 会提示"未知发布者"，
这是预期行为（开源项目常见）。

我们正在申请 [SignPath Foundation](https://signpath.org/) 的免费开源代码签名，
通过后 release 将由 SignPath Foundation 签名。详见 [docs/SIGNING.md](docs/SIGNING.md)。

## 贡献

欢迎 issue 与 PR，请先读 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 许可证

[MIT](LICENSE) © 2026 XuJake
