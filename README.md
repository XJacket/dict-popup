# dict-popup

> **测试版（0.2.x）** —— 功能可用但仍在验证中，接口与行为可能调整。
> 首个公开版本为 `0.2.1`，未签名；签名与 1.0 计划见 [代码签名](#代码签名)。

**离线划词词典，Windows 桌面小工具。** 选中任意程序里的文字，按一次热键，鼠标旁弹出释义卡片。
不联网、不启动 GoldenDict、单文件 exe、零运行时依赖。

**为什么做这个工具** 作者本人是一个英语学习者，在浏览网站时经常被不认识的单词卡住，想要一款好用的划词翻译工具。而现在市面上的主流工具都使用 离线 + 调用商用翻译接口 的形式实现，不但速度缓慢，影响阅读体验，还不提供单词的详细释义。于是我使用vibecoding构建了这个轻量化的翻译工具。它响应迅速，完全离线，并能显示全面的词典释义。

> Offline word-lookup popup for Windows. Select text anywhere, press a hotkey, get a
> floating card with the definition. No network, no runtime, one exe.

## 为什么选择dict-popup

- **完全离线**：直接读取本地词典，不联网、不上传任何内容。
- **解释详细**：查词结果包括完整词条与例句，适合作为语言学习者的学习工具。
- **轻量**：release 单 exe 约 6.5 MB，私有内存 55–65 MB，空闲工作集 3–14 MB
  （实测数据见 [docs/STATUS.md](docs/STATUS.md)）。
- **快速**：340 万词条的词典，单次查询 5–12 µs；打开词典（含建索引）约 80 ms。
- **悬浮卡片**：无边框、半透明、置顶、跟随鼠标出现，不会打断你正在做的事。

## 安装与配置

1. 从 [Releases](../../releases) 下载 `dict-popup-0.2.1-win64.zip`（也可以从源码构建，见下节）
2. 解压到**任意普通目录**（例如 `C:\Users\<you>\Apps\dict-popup`）
   - ⚠️ 不要把 exe 放在被系统标记为「低完整性」的目录里（某些沙箱/受限目录），
     否则 Windows 会阻止它向其他程序发送按键，取词会失败
3. 运行 `dict-popup.exe`，首次启动会打开设置窗口

> **未签名**：`0.2.1` 没有公共 CA 的证书，Windows 会提示"未知发布者"或"Windows 已保护你的电脑"。
> 这是未签名开源软件的预期行为，可点「更多信息 → 仍要运行」。
> 若在**普通权限**下取不到词，或需要向提权窗口取词，用下面的**本地自签名安装**启用 UIAccess。

### 本地自签名安装（启用 UIAccess）

官方 release 暂无证书。如果你希望**在提权程序里也能取词**，可以在本机自签一次——
自签证书同样满足 UIAccess 的签名要求，只是**只对这台机器有效**。

**前提**：Rust（GNU 工具链）+ MinGW（`build.bat` 需要）；一个**管理员 PowerShell**。

**一条命令**（构建 → 自签 → 信任到本机 → 安装）：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\setup-uiaaccess-local.ps1
```

它依次做五步：

| 步骤 | 动作 |
|---|---|
| 1 | 检查是否提权（没提权立即退出，不写任何东西） |
| 2 | `build.bat /uiaccess` 构建带 `uiAccess="true"` 的变体 |
| 3 | `scripts\sign-release.ps1` 生成自签代码签名证书并签名 |
| 4 | 把证书导入**本地计算机**的「受信任的根证书颁发机构」 |
| 5 | 复制到 `%ProgramFiles%\dict-popup\`，并创建开始菜单快捷方式 |

**分步执行**（想手动控制时）：

```powershell
cmd /c build.bat /uiaccess
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\sign-release.ps1
#  以管理员身份把 assets\signing\dict-popup-codesign.cer 导入
#  「受信任的根证书颁发机构（本地计算机）」
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-uiaaccess.ps1
```

**验证**（从开始菜单启动，**不要**右键「以管理员身份运行」）：

1. 设置 → 关于，应显示 `UIAccess: 已启用 · 完整性: High`
2. `%APPDATA%\dict-popup\dict-popup.log` 应出现 `privilege: uiaccess=true ...`
3. 决定性测试：在**以管理员身份运行**的记事本里选中单词，按热键，应弹出卡片

> `完整性` 显示 **High** 才是生效；显示 `Medium` 说明 UIAccess 没拿到。

**安全说明（请读完再决定）**

- 自签证书只写进**你这台机器**的根证书存储，其他机器不信任它；
  **不要把这个签名后的 exe 分发给别人**——对方机器上它会直接拒绝启动（`ERROR_ELEVATION_REQUIRED`, 740）。
- 这台机器的「受信任的根证书颁发机构」会多出一条 `CN=XuJake`。如果你不接受这个副作用，就不要启用 UIAccess，
  改用普通解压版（代价是提权窗口里取不到词）。
- 重新构建 exe 后，必须**重新签名并重新安装**一次。

**卸载 / 撤销信任**（管理员 PowerShell）：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\install-uiaaccess.ps1 -Uninstall

# 删除本机根证书存储里的自签证书
Get-ChildItem Cert:\LocalMachine\Root, Cert:\LocalMachine\TrustedPublisher |
    Where-Object { $_.Subject -eq 'CN=XuJake, O=XuJake, C=CN' } |
    ForEach-Object {
        $name = $_.PSParentPath.Split('::')[-1].Split('\')[-1]
        $store = New-Object System.Security.Cryptography.X509Certificates.X509Store($name, 'LocalMachine')
        $store.Open('ReadWrite'); $store.Remove($_); $store.Close()
        Write-Host "removed $($_.Thumbprint) from $name"
    }
```

不使用 UIAccess 时，按上面的普通步骤解压运行即可，不需要管理员权限。详见 [docs/UIAACCESS.md](docs/UIAACCESS.md)。

## 首次配置

设置 → 词典 → 添加 `.ifo` 文件，指向你的 StarDict 词典，例如：

```
D:\dicts\ecdict\stardict-ecdict-2.4.2.ifo
```

需要 `.ifo` + `.idx` + 未压缩的 `.dict` 三个文件，放在同一目录。
词典可以从 [ECDICT](https://github.com/skywind3000/ECDICT) 转换而来。

若用了上面的自签名安装，顺便看一眼 **设置 → 关于** 里的 `UIAccess` 状态：
显示「已启用」表示提权窗口里也能取词；显示「未启用」则与普通解压版行为一致。

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

**验证状态（0.2.1，如实说明）**

- 取词只在 **Windows 11 记事本**里完成过实机验证（`Ctrl+C` 路径）；
  Chrome / Office / PDF / Electron 应用**尚未逐项验证**
- 托盘图标在部分系统上不显示，尚未定位（[issue](.scratch/dict-popup/issues/01-tray-icon-missing.md)）
- **以普通权限运行时可能取不到词**：这是 UIPI 拦截——程序的完整性级别低于目标窗口时，
  系统会静默丢弃合成输入。已确认的解法是 [UIAccess](docs/UIAACCESS.md)：签名 + 安装到
  `%ProgramFiles%` + manifest 声明，进程会被提升到 High 完整性级别并跨过该限制
  （2026-10-25 实测：非提权启动、`uiaccess=true`、在提权记事本里成功取词）。
  未签名的 `0.2.1` 版本**不包含**这条路径，所以仍可能失败

**环境要求**

- 不要放在被标记为「低完整性」的目录（沙箱、部分受限目录），否则系统会丢弃合成输入

## 代码签名

本项目的 release 目前**没有公共 CA 的证书**：Windows 会提示"未知发布者"，这是预期行为。

- **自用推荐**：按[本地自签名安装](#本地自签名安装启用-uiaccess)在本机签一次，
  既消除提示，也能启用 UIAccess（需要签名才能用）。详见 [docs/UIAACCESS.md](docs/UIAACCESS.md)。
- **公开分发**：公共 CA 证书（OV/EV，约 $300–500/年）或
  [SignPath Foundation](https://signpath.org/) 的免费开源签名（申请中，暂时拿不到）。
  详见 [docs/SIGNING.md](docs/SIGNING.md)。

自签名**只对签名的那台机器有效**，不要把它当作公开分发的替代品。

## 贡献

欢迎 issue 与 PR，请先读 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 许可证

[MIT](LICENSE) © 2026 XuJake
