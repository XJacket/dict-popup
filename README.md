# dict-popup

**离线划词词典。** 选中任意程序里的文字，按一次热键，鼠标旁弹出释义卡片。
不联网、不启动 GoldenDict、**不碰剪贴板**，单文件 exe，解压即用。

- 取代 `legacy/goldendict-uia-hotkey`（旧工具只负责把词丢给 GoldenDict）。
- 词典用本地 [StarDict](https://en.wikipedia.org/wiki/StarDict) 格式（ECDICT 等）。
- 取词走 Windows UI Automation，通过 `UIA_TextPattern` 读取选区，不发送合成按键。

## 安装

1. 解压 `dict-popup-0.1.0-win64.zip` 到任意目录（例如 `D:\Tools\dict-popup`）。
2. 双击 `dict-popup.exe`。首次启动没有词典，托盘会出现图标并提示去配置。
3. 程序**没有安装程序**，也不写注册表（只有“开机自启”会写当前用户的 `Run` 项）。

发布包内容：

| 文件 | 说明 |
|---|---|
| `dict-popup.exe` | 主程序（图标 + 版本资源已嵌入，无外部 DLL） |
| `README.md` | 本文件 |
| `LICENSE` | MIT 许可证 |
| `CHANGELOG.md` | 版本变更记录 |
| `config.example.json` | 配置示例，可照着改 `%APPDATA%\dict-popup\config.json` |

要求：Windows 10/11 x64。不需要 .NET、不需要 Visual C++ 运行库、不需要 Rust。

## 首次使用

1. 右键托盘图标 → **设置**。
2. 打开 **词典** 页 → **添加**，选择 StarDict 词典的 `.ifo` 文件，例如：

   ```
   D:\program\GoldenDict-ng\dicts\ecdict\stardict-ecdict-2.4.2.ifo
   ```

   同一目录下的 `.idx`、`.dict` / `.dict.dz` 会被自动找到，**不要**单独添加它们。
3. 添加后列表会显示词条数、识别出的方向（英→中 / 中→英）和置信度。
   置信度低于 0.85 时标黄，提示“自动识别不确定”，可在该行手动指定方向。
4. 关掉设置窗口即可生效（不需要重启）。

配置文件在 `%APPDATA%\dict-popup\config.json`，首次运行自动生成；
保存时先写临时文件再原子替换，并保留 `config.json.bak`。
配置解析失败时程序**不会**覆盖你的文件，而是提示你处理。
可参考同目录的 `config.example.json`（字段与 `docs/DESIGN.md` §4 一致）。

## 热键

- 默认 **`Ctrl+Alt+D`**：取词并弹出卡片。
- 在 **设置 → 热键** 里点一下捕获框，直接按下新组合键即可修改；非法组合会说明原因。
- 新热键注册成功后才释放旧的；失败会保留旧热键并报错（不会出现“热键没了”）。
- 与其它程序冲突时 Windows 会拒绝注册，设置页会给出中文提示。

## 卡片操作

- 卡片出现在鼠标右下方，碰到屏幕边缘会自动翻转，按当前显示器的 DPI 定位。
- 顶部是词条、音标、方向徽章和来源词典；正文按词典分组，**第一组展开**，其余折叠并显示命中条数。
- 正文里的文字**可以拖选复制**。
- 关闭：`Esc`、点击卡片外、失焦（可在设置里关掉）、或**再按一次热键收起**。
- 连续按热键只会替换卡片内容，不会叠开第二个窗口。
- 未收录时显示“未收录：<词>”，并列出最多 3 个尝试过的候选写法。

## 设置说明

| 分节 | 内容 |
|---|---|
| 热键 | 捕获组合键、恢复默认 |
| 词典 | 拖动排序、启用/禁用、删除、添加 `.ifo`；显示方向、置信度、词条数；文件丢失或格式不支持会标红 |
| 外观 | 主题（跟随系统 / 浅色 / 深色）、字号、卡片宽度 |
| 行为 | 开机自启、去掉选中文字首尾空白、失焦关闭、自动隐藏秒数（0 = 不自动隐藏） |
| 关于 | 版本、日志目录、配置路径、许可证 |

保存即生效。

## 限制

- **仅支持 StarDict**（`.ifo` + `.idx` + `.dict`/`.dict.dz`）。不支持 MDX/MDD、GoldenDict 自有格式、在线词典。
- **没有发音**（不接 TTS，也不播放在线音频）。
- **没有模糊搜索 / 拼写建议**：只查规范化后的候选与词条精确匹配（大小写、全半角等规范化由 `dict-core` 负责）。
- 取词依赖目标程序的 UI Automation 支持：Chrome/Edge（需启用无障碍）、记事本、Word、VS Code、多数 PDF 阅读器可用；
  部分游戏、自绘界面、以管理员权限运行的程序取不到词，会给出提示而不是崩溃。
- 只支持 Windows 10/11 x64。
- 日志**绝不记录**选中的文字内容，只记录长度和命中的词典名。

## 从源码构建

依赖：Rust（GNU host，`x86_64-pc-windows-gnu`）+ MinGW-w64 的 `gcc`、`windres`。
仓库自带的工具链在 `tools\mingw64\bin`，`build.bat` 会自动加进 `PATH`。
不需要 MSVC、.NET SDK 或 Windows SDK。

```bat
build.bat
```

产物：`dist\dict-popup.exe`，图标与 `VERSIONINFO` 已嵌入。构建脚本会检查版本资源，缺失就报错。

打包发布（文件夹 + zip）：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\package.ps1
```

产出：

```
dist\dict-popup-0.1.0-win64\
dist\dict-popup-0.1.0-win64.zip
```

内存验收（默认不启动程序；加 `-Launch` 才会启动并在测完后关闭）：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\measure-memory.ps1 -Launch
```

重新生成图标（需要 `System.Drawing`，Windows 自带）：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\make-icon.ps1
```

### 资源是怎么进 exe 的

`cargo build` 不接受额外的链接参数。`build.bat` 用

```bat
windres -I assets --codepage=65001 -O coff -o assets\dict-popup.res assets\version.rc
cargo rustc --release --manifest-path dict-popup\Cargo.toml --bin dict-popup -- -C link-arg=F:/path/to/dict-popup/assets/dict-popup.res
```

把 `.res` 交给 GNU 链接器，只重新链接最终二进制，不重建依赖。
如果希望 `cargo build`（以及 IDE、`cargo run`）也自动带资源，需要改的是界面层目录
（发行工程不替它改）：`dict-popup\Cargo.toml` 的 `[package]` 加一行 `build = "build.rs"`，
再放一个 `dict-popup\build.rs`：

```rust
// dict-popup/build.rs
use std::process::Command;

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate lives in <repo>/dict-popup");
    let res = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("dict-popup.res");
    let status = Command::new("windres")
        .arg("-I").arg(root.join("assets"))
        .args(["--codepage=65001", "-O", "coff", "-o"]).arg(&res)
        .arg(root.join("assets/version.rc"))
        .status().expect("windres not found in PATH");
    assert!(status.success(), "windres failed");
    println!("cargo:rustc-link-arg-bins={}", res.display());
    println!("cargo:rerun-if-changed={}", root.join("assets/version.rc").display());
    println!("cargo:rerun-if-changed={}", root.join("assets/dict-popup.ico").display());
}
```

以 `build = "build.rs"` 为开关，这样 `cargo build` 才会自动带图标和版本信息；`build.bat`
本身不依赖它。

## 许可证

MIT，见 [LICENSE](LICENSE)。Copyright (c) 2026 XuJake。
