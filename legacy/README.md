# legacy — 旧项目，仅作参考

这里保存 `goldendict-uia-hotkey` 的**源码快照**（`src/`、`Cargo.toml`、`Cargo.lock`、
`build.bat`、`README.md`），它被 `dict-popup` 完全取代。

## 为什么不整个搬过来

原始目录 `F:\project\translation\goldendict-uia-hotkey\` 仍然留在工作区根目录。
尝试把它移进本目录时被系统拒绝（`Move-Item`、`Directory.Move`、`cmd move` 三种方式
都返回 Access denied）：那个目录缺少当前会话令牌的删除权限，而工作区根目录上还存在
一条"Everyone 拒绝删除子项"的规则。文件本身没有任何损坏。

所以：**快照在这里被 git 跟踪，原目录不跟踪**（见根目录 `.gitignore`）。
原目录里 240 MB 的 `target/` 是构建产物，删掉也不影响任何东西。

## 参考价值

- `src/main.rs`：热键注册、UIA 取词、托盘图标、单窗口消息循环的完整 Win32 写法，
  新项目里这些管道会复用同样的思路（但会挪到独立工作线程，并修掉已知缺陷）。
- `README.md`：原始功能说明和已知限制。
