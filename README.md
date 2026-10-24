# 翻译相关文件

这个目录集中存放本机与翻译相关的项目、构建工具和截图。

## 目录

| 路径 | 说明 |
|---|---|
| `goldendict-uia-hotkey/` | Rust 写的全局热键小工具：UIA 读取选中文字，调用 GoldenDict 查词，不经过剪贴板 |
| `tools/mingw64/` | 构建上面 Rust 项目用的 MinGW-w64 工具链 |
| `screenshots/` | GoldenDict / Pot 的验证截图 |

## 已安装的翻译程序（不在本目录）

| 程序 | 位置 | 说明 |
|---|---|---|
| GoldenDict-ng | `D:\program\GoldenDict-ng` | 离线词典，ECDICT 340 万词条 |
| Pot | `D:\program\Pot` | 在线词典/翻译，已配置剑桥词典 |

## 小工具用法

运行：

```
goldendict-uia-hotkey\target\release\goldendict-uia-hotkey.exe
```

选中文字后按 `Ctrl + Alt + D`，用 GoldenDict 查询。

配置：`%APPDATA%\GoldenDictUiaHotkey\config.json`，可改热键和 GoldenDict 路径。

## 重新构建

```
cd goldendict-uia-hotkey
build.bat
```

Rust 工具链在 `C:\Users\xujia\.rustup`，MinGW 在本目录 `tools\mingw64`。
