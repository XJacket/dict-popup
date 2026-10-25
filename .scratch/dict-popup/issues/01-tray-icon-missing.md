# 01 — 托盘图标从未显示

- **Type:** bug
- **Status:** ready-for-agent
- **Reported:** 2026-10-25（用户实机反馈）
- **Area:** dict-input / tray

## 现象

程序启动后，Windows 11 通知区域看不到 dict-popup 图标（折叠区 `^` 里也没有）。
用户无法通过托盘菜单打开设置或退出，只能用任务管理器结束进程。

## 验证（已确认）

查 `HKCU\Control Panel\NotifyIconSettings`：**没有任何 ExecutablePath 指向
dict-popup 的条目**。Windows 11 会给每一个真正注册过的托盘图标建一条记录，
被折叠进溢出区的图标同样有记录。没有记录 ⇒ `Shell_NotifyIcon(NIM_ADD)`
从未成功过，而不是"图标被隐藏了"。

## 代码路径

`dict-input/src/host_impl/tray_impl.rs`：

- `attempt()` 调 `Shell_NotifyIconW(NIM_ADD, ...)`，失败后靠 `SetTimer` 重试
  10 次 × 500 ms；
- 10 次都失败后**静默放弃**——没有事件、没有日志、没有气泡，界面层完全不知道
  托盘不可用；
- 图标句柄来自 `load_app_icon()`：先 `LoadImageW(hinstance, id=1)`，失败回退
  `IDI_APPLICATION`。

可疑点（按怀疑度排序）：

1. `LoadImageW` 的 `LR_DEFAULTSIZE` 与 `cx=cy=0` 组合在某些 resource 配置下
   返回 NULL，随后回退也可能失败 → `hIcon` 为 NULL 时 `NIF_ICON` 仍然置位，
   部分 shell 版本会直接让 `NIM_ADD` 失败；
2. `NIM_ADD` 的失败原因从未被记录（`Shell_NotifyIconW` 只返回 BOOL，具体
   `GetLastError` 没读）；
3. `NIM_SETVERSION` 的返回也未检查，v4 失败时某些消息语义会变。

## 验收标准

1. 托盘图标在 Win11 默认配置下可见（允许被折叠进 `^`，但必须有条目）；
2. 右键菜单三项可用（设置 / 重载词典 / 退出）；
3. `Shell_NotifyIcon` 失败时必须可见：托盘气泡或设置页错误行 + 日志；
4. Explorer 重启后图标自动恢复（已有 `TaskbarCreated` 逻辑，需回归）；
5. 无法加载 exe 内图标时回退到系统图标仍能显示。

## 起点

先让 `attempt()` 记录 `GetLastError()` 并暴露一次失败事件（`TrayFailed`），
用真实环境跑一次拿到失败码，再决定修图标加载还是修注册时机。
