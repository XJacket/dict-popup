# GoldenDict 划词助手

一个 Windows 托盘小工具：按下自定义全局热键，用 **Windows UI Automation** 读取当前选中的文字，然后调用 GoldenDict 查询。

**不经过剪贴板**，不会覆盖你复制的内容。

## 直接运行

```
target\release\goldendict-uia-hotkey.exe
```

启动后没有主窗口，只在系统托盘出现图标。默认热键：`Ctrl + Alt + D`。

## 使用

1. 在任意程序里选中一个英文单词或短语；
2. 按 `Ctrl + Alt + D`；
3. GoldenDict 打开并查询选中的内容。

托盘图标右键菜单：

- 打开 GoldenDict
- 打开配置文件
- 重载配置
- 退出

## 配置

配置文件位置：

```
%APPDATA%\GoldenDictUiaHotkey\config.json
```

默认内容：

```json
{
  "hotkey": "Ctrl+Alt+D",
  "goldendict_path": "D:\\program\\GoldenDict-ng\\goldendict.exe",
  "trim": true
}
```

- `hotkey`：热键，支持 `Ctrl` / `Alt` / `Shift` / `Win` 组合，例如 `Ctrl+Alt+D`、`Alt+Shift+Q`、`F9`；
- `goldendict_path`：GoldenDict 可执行文件路径；
- `trim`：是否去掉选中文字首尾空白。

改完保存，托盘菜单点 **重载配置** 即可生效；如果热键被占用，会提示注册失败。

## 限制

- 依赖目标程序通过 UI Automation 暴露选中的文本：Chrome/Edge、Office、多数现代应用可用；部分老程序、游戏、Java 应用读不到，此时会响一声提示音。
- 这是“选中文字 + 按热键”，不是鼠标划选即弹；GoldenDict Windows 版本身没有鼠标取词。
- 只做取词和调用 GoldenDict，不做句子翻译。

## 从源码构建

需要 Rust GNU 工具链和 MinGW-w64。当前机器上的路径：

- Rust：`C:\Users\xujia\.rustup\toolchains\stable-x86_64-pc-windows-gnu`
- MinGW：`F:\project\translation\tools\mingw64`

运行：

```bat
build.bat
```

产物在 `target\release\goldendict-uia-hotkey.exe`。

## 开机自启

把桌面的 `GoldenDict 划词助手.lnk` 复制到：

```
%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup
```

## 日志

排查问题时看：

```
%APPDATA%\GoldenDictUiaHotkey\app.log
```

会记录热键注册、UIA 读取到的文字长度、调用 GoldenDict 的结果。
