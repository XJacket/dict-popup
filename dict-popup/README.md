# dict-popup

离线划词词典：按自定义热键 → 读取选中文字 → 查本地 StarDict 词典 → 在鼠标旁弹出悬浮卡片。

取代 `legacy/goldendict-uia-hotkey`，不再启动 GoldenDict 进程。

## 状态

Stage 0：UI 技术栈冒烟验证（egui/eframe）。功能链路尚未接入。

## 构建

首次需要一个联网步骤（本机 agent 会话无法访问 crates.io）：

```
F:\project\translation\fetch-deps.bat
```

之后全部离线：

```
cd F:\project\translation\dict-popup
set CARGO_HOME=F:\project\translation\.cargo-home
set PATH=F:\project\translation\tools\mingw64\bin;%PATH%
cargo build --release --offline
```

## 设计约束

- 内存 ≤ 100 MB，单 exe，无运行时依赖。
- 词典数据：StarDict（ECDICT），mmap + 二分查找，不转格式、不复制。
- 每本词典带语言方向（英→中 / 中→英）；切换方向 = 切换参与查询的词典集合。
- 第一版支持同时加载多本词典，结果按词典分组显示。
