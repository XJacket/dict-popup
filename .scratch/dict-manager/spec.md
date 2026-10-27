# 词典管理工具 — 设计草案

- **分支**：`feat/dictionary-manager`
- **状态**：草案（实现待 `docs/DICTIONARIES.md` 调研结论落定）
- **目标**：让 dict-popup 能管理"不止一本"词典，并且为将来的在线获取留好接口

## 问题

现在词典是配置里的一串路径（`config.json → dictionaries[].path`）：

- 没有"词典包"的概念，用户得自己找文件、解压、记路径
- 没有版本：升级一本词典 = 手动覆盖文件，出问题无法回退
- 没有来源与许可记录：不知道这本词典从哪来、能不能再分发
- 无法预知"有没有新版本"——因为没有元数据

## 决定（待确认）

引入 **词典库（library）** 概念：一个目录 + 一个清单文件，每本词典是一个**包**。

```
%APPDATA%\dict-popup\library\
  library.json          # 清单：包列表 + 版本历史
  ecdict-2.4.2\         # 一个包一个目录，目录名 = <id>-<version>
    source.json         # 该包的元数据（与清单同源）
    stardict-ecdict.ifo
    stardict-ecdict.idx
    stardict-ecdict.dict
```

### 包元数据

| 字段 | 说明 |
|---|---|
| `id` | 稳定标识，如 `ecdict`（同一本词典跨版本不变） |
| `version` | 语义化版本或数据源日期（`2024.03` 亦可），用于比较新旧 |
| `format` | `stardict` / `jsonl` / `mdx` …（决定用哪个读取器） |
| `name` | 显示名 |
| `license` | SPDX 标识或自由文本；**必填**，缺失则拒绝入库 |
| `homepage` | 来源主页 |
| `sha256` | 包内容摘要，用于校验与判定"内容是否真的变了" |
| `bytes` / `entries` | 体积与词条数（后者可空） |
| `installed_at` | 安装时间 |
| `active` | 是否参与查询 |

### 版本控制语义

- **升级 = 新增一个版本目录再切换 active**，旧版本目录保留（可回退），除非显式 `prune`
- 比较规则：`version` 更高 **且** `sha256` 不同 → 升级；`version` 相同但 `sha256` 不同 → 视为"同版本内容变更"，要求显式 `--force`，避免数据源悄悄改内容
- 历史事件（`installed` / `upgraded` / `removed` / `activated` / `deactivated`）追加到 `library.json`，只追加不修改
- 回退：`rollback <id>` 把 active 指回上一个版本目录

### 预留的下载接口

```rust
/// 一个"能拿到词典包"的地方。现在只有本地目录实现；将来是 HTTP。
pub trait Provider {
    fn describe(&self) -> ProviderInfo;          // 名称、是否需要网络
    fn latest(&self, id: &SourceId) -> Result<RemotePackage, ProviderError>;
    fn fetch(&self, remote: &RemotePackage, dest: &Path) -> Result<(), ProviderError>;
}
```

- 现在实现 `LocalDirectoryProvider`（从一个目录扫描 `*.zip` / 目录）
- `HttpProvider` 只留骨架 + `todo!()` **不可达**（`describe().available == false`），避免半成品联网
- 测试用 `FakeProvider`，证明 `latest`/`fetch` 的调用顺序与失败处理

## 模块与 seam

新 crate `dict-store`（不塞进 dict-popup，便于独立测试）：

```
dict-store/src/
  lib.rs        Library、DictionaryPackage、SourceId、Format
  version.rs    版本比较（纯函数）
  manifest.rs   library.json 读写（原子写、坏文件不覆盖）
  checksum.rs   目录/文件摘要
  provider.rs   Provider trait + LocalDirectoryProvider + 预留 HttpProvider
  error.rs      StoreError
  bin/dict-store.rs  CLI: list / add / upgrade / rollback / remove / activate
```

`Library` 的接口保持小而深：`open` / `list` / `add` / `upgrade` / `rollback` / `remove` / `set_active` / `history`。
文件系统、校验、清单合并都在实现里，调用方（CLI 或 UI）只谈"包"。

## 与现有代码的关系

- `dict-popup` 的 `Registry` 现在按 `config.dictionaries[].path` 加载。**本次不改查询路径**，
  避免把两个变更混在一起；`dict-store` 先把"包"管好，下一步再让 Registry 从 library 读取。
- 因此本次交付：crate + CLI + 测试 + 文档；UI 集成列为后续任务。

## 验收

1. `cargo test --release --manifest-path dict-store\Cargo.toml` 全绿（含 fs 测试，用 `target/test-tmp`）
2. 命令行可完成：添加一本词典目录 / 列表 / 升级 / 回退 / 删除 / 启停
3. `library.json` 原子写入；解析失败不覆盖用户文件
4. Provider 抽象有 fake 测试；`HttpProvider` 不可达且不会发网络请求
5. 文档说明"支持哪些格式"以 `docs/DICTIONARIES.md` 的结论为准
