# dict-store — 词典库（包、版本、校验、历史）

`dict-store` 管理"已安装的词典"，不负责查词、不做界面、不解析词典文件本身。

它是独立 crate（自己的 `Cargo.toml`，edition 2021，不依赖 `dict-core` /
`dict-input` / `dict-popup`，根目录也没有 workspace 清单）。`dict-popup` 的查询路径
暂时不动，后续再让 `Registry` 从词典库读取启用中的版本。

## 词典库布局

一个目录就是一本"库"：

```text
<root>/
  library.json              # 清单：包记录 + 只追加的历史
  ecdict-2.4.2/             # 一个已安装版本一个目录，目录名 <id>-<version>
    source.json             # 该版本的元数据副本（与清单同源）
    stardict-ecdict.ifo
    stardict-ecdict.idx
    stardict-ecdict.dict
  ecdict-2.4.1/             # 旧版本保留，供 rollback
```

`Library::open(root)` 只读取和校验清单，**不创建任何东西**；根目录和
`library.json` 在第一次写入时才出现。

### 清单示例

```json
{
  "schema": 1,
  "packages": [
    {
      "id": "ecdict",
      "version": "2.4.2",
      "format": "stardict",
      "name": "ECDICT",
      "license": "CC-BY-SA-4.0",
      "homepage": "https://example.invalid/ecdict",
      "sha256": "…64 位小写十六进制…",
      "bytes": 199229440,
      "entries": 770000,
      "installed_at": "2026-10-27T05:31:12.345Z",
      "active": true
    }
  ],
  "history": [
    {
      "at": "2026-10-27T05:31:12.345Z",
      "event": "installed",
      "id": "ecdict",
      "version": "2.4.2"
    }
  ]
}
```

- `license` **必填且不能为空**：许可证不明的包拒绝入库。
- `installed_at` 固定为 UTC 的 RFC 3339、毫秒精度（`2026-10-27T05:31:12.345Z`）。
- `format` 是开放式标签：`stardict` / `jsonl` / `mdx` 有名字，其它名字原样保留。

## 版本规则

比较版本用 `version::compare`（纯函数），规则见下节。`add` / `upgrade` /
`rollback` / `remove` / `activate` / `deactivate` 都会向 `history` 追加事件，
历史**只追加、不修改**。

| 操作 | 行为 |
|---|---|
| `add` | 安装一个版本目录。同一个 `(id, version, sha256)` 永远拒绝（`--force` 也不行）；同 `(id, version)` 但内容不同需 `--force`；`<id>-<version>` 目录已存在但清单无记录也需 `--force`。该 id 的第一个版本自动 `active`，之后的版本装成未启用。 |
| `upgrade` | 安装新版本目录，把该 id 其它版本标为未启用、新版本标为启用，追加 `upgraded` 事件。新版本必须比当前最新版本更高，否则需 `--force`。旧版本目录保留。 |
| `rollback` | 把 `active` 指回该 id 上一个（小于当前启用版本的）已安装版本，追加 `rolled_back` 事件。没有更旧版本时报告错误。 |
| `remove` | 删除**指定的那一个**版本目录（省略版本则删除该 id 的全部版本），每个版本追加一条 `removed`。删掉的是启用版本时，自动启用剩下的最新版本；没有剩余版本则该 id 不再有启用版本。 |
| `activate` / `deactivate` | 显式切换 / 停用某 id 的启用状态。`deactivate` 时若本来就没启用版本，不写事件。 |

同一 id 任一时刻最多一个 `active` 版本。

### 版本比较规则

1. 在**第一个** `-` 处切成 release 和 pre-release：`1.2.0-rc.1` → `1.2.0` / `rc.1`。
2. release 按 `.` 分段比较：
   - 两段都是纯数字 → 按数值比较（`03` 与 `3` 相等，`10` 大于 `9`，超长数字不会溢出）；
   - 否则按 ASCII、忽略大小写逐字节比较；
   - 缺少的段当作 `0`，所以 `1.0` 等于 `1.0.0`，小于 `1.0.1`。
3. release 相等时：没有 pre-release 的更大（`1.0.0` > `1.0.0-rc.1`）；两者都有则按 `.` 分段比较：
   数字段按数值、数字段小于非数字段（`1.0.0-2` < `1.0.0-rc`）、非数字段忽略大小写比较；
   前缀相同时段数少的更小（`1.0.0-rc` < `1.0.0-rc.1`）。
4. 全部相等即相等。

所以 `2024.03 < 2024.10`、`2024.3 == 2024.03`、`2.4.2 > 2.4.1`、`1.10 > 1.9` 都成立。
该顺序是全序，用来确定"最新版本"。

## 校验（sha256）规则

`checksum::sha256_dir` 对包目录计算摘要，`library.json` 里记录的 `sha256` 就是它：

1. 递归遍历目录；**所有普通文件**都参与，唯一例外是**顶层** `source.json`
   （库自己的元数据副本，写入时摘要已经算完，算进去会变成循环依赖）。子目录里的
   `source.json` 是普通包文件，照常参与。
2. 文件的相对路径统一用 `/` 分隔（Windows 的 `\` 归一化），UTF-8 编码。
3. 按相对路径的字节序排序。
4. 用**同一个** SHA-256 依次喂入每个文件：相对路径字节 → 一个 NUL 字节（`0x00`）→
   文件内容（按 64 KiB 分块）。
5. 结果是 32 字节摘要的小写十六进制。

因为先喂路径和 NUL 分隔符，改名会改变摘要；因为先排序，摘要与目录列举顺序、创建顺序、
时间戳、绝对路径都无关——把包原样拷到别处摘要不变。空目录（或只有顶层
`source.json` 的目录）的摘要等于空输入的 SHA-256：
`e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`。

`bytes` 是这些参与摘要的文件大小之和。

## 原子性与坏文件保护

- `library.json` 先写到同目录的临时文件（`.library.json.tmp-<pid>`），`sync_all`
  之后 `rename` 覆盖目标：崩溃时要么是旧文件、要么是新文件，不会出现半个 JSON。
- 如果已存在的 `library.json` **解析失败**，或本程序无法使用它（例如 `schema` 比本
  程序新），写入直接返回错误、**不覆盖**原文件——用户还能手工修复；只有删掉/修好它，
  写入才会继续。
- 安装目录先写到 `.staging-<id>-<version>-<pid>`，完整后再 `rename` 成正式目录；
  若清单写入失败，新目录被移除、被替换的旧目录被还原。
- 删除版本时先把目录改名成 `.trash-…`，清单写成功后才真正删除；写入失败则改回来。

## CLI

```text
dict-store [--root <目录>] <命令> [参数]

list                                    列出已安装版本
add <目录> --id --version --name --license [--format --homepage --entries --force]
upgrade <目录> --id --version --name --license [--format --homepage --entries --force]
rollback <id>
remove <id> [--version <版本>]
activate <id> <版本>
deactivate <id>
history [id]
```

`--root` 是全局选项，默认值是 **`dict-store.exe` 所在目录**（`library.json` 与各版本
目录都放那里）。实际使用时建议显式给出，例如 `%APPDATA%\dict-popup\library`。

`add` / `upgrade` 的必填字段也可以写在包目录的 `source.json` 里，命令行给出的值优先。
未知格式不需要改代码：`--format` 或 `source.json` 里的任意名字都会原样记录。

示例输出：

```text
> dict-store --root D:\dicts list
词典库：D:\dicts（2 个版本）
ID      VERSION  ACTIVE  FORMAT    ENTRIES  SIZE  NAME    LICENSE
------  -------  ------  --------  -------  ----  ------  -------
ecdict  1.0      *       stardict  -        3 B   ECDICT  MIT
ecdict  2.0              stardict  -        3 B   ECDICT  MIT

> dict-store --root D:\dicts upgrade .\ecdict-2.1 --id ecdict --version 2.1 --license MIT
已升级 ecdict 2.0 → 2.1（ECDICT）
  目录：D:\dicts\ecdict-2.1
  sha256：… · 7 B · 启用中

> dict-store --root D:\dicts history
词典库 D:\dicts 的历史记录（4 条）
TIME                      EVENT        ID      VERSION  DETAIL
------------------------  -----------  ------  -------  ----------
2026-10-27T05:37:20.466Z  installed    ecdict  1.0
2026-10-27T05:37:20.543Z  installed    ecdict  2.0
2026-10-27T05:37:25.388Z  upgraded     ecdict  2.1      2.0 -> 2.1
2026-10-27T05:37:25.524Z  rolled_back  ecdict  2.0      2.1 -> 2.0
```

失败时向 stderr 打印中文错误并以非零状态退出；`--help`（以及每个子命令的 `--help`）
打印用法并以 0 退出。

## Provider：包从哪里来

`provider` 模块是预留的下载 seam：

- `LocalDirectoryProvider` 扫描一个目录，找出含 `source.json` 的子目录，按版本返回最新
  的一个，并已实现、已测试（含 `fetch` 复制到目标目录）。
- `HttpProvider` 只是**骨架**：`describe().available == false`，`latest` / `fetch` 一律返回
  `ProviderError::Unavailable("network provider not implemented")`，不发起任何网络请求；
  本 crate 也没有网络依赖。它是"将来的下载接口"占位。

两段式接口（先 `latest` 再 `fetch`）让"有没有新版本"可以只看元数据、不下载。

## 模块

| 文件 | 内容 |
|---|---|
| `src/lib.rs` | `Library`、`DictionaryPackage`、`SourceId`、`Format`、时间戳 |
| `src/version.rs` | 版本比较、版本字符串校验（纯函数） |
| `src/manifest.rs` | `library.json` 读写与校验、`HistoryEvent`、`PackageMeta`、`NewPackage` |
| `src/checksum.rs` | 文件 / 目录 sha256、包目录遍历 |
| `src/provider.rs` | `Provider` trait、`LocalDirectoryProvider`、`HttpProvider` 骨架 |
| `src/error.rs` | `StoreError`（带路径 / id 上下文，中文 `Display`） |
| `src/bin/dict-store.rs` | CLI |

## 依赖

只有 `serde`（derive）、`serde_json`、`sha2`。CLI 参数是手写的（接口很小，省掉 clap
及其 8 个传递依赖，帮助文本也可控）。

## 测试

```bat
cargo test --release --manifest-path dict-store\Cargo.toml
cargo run --release --manifest-path dict-store\Cargo.toml --bin dict-store -- --help
```

文件系统测试的临时目录一律建在 `target/test-tmp/<唯一名字>`（不写 `%TEMP%`，本机
子进程写不了那里），并在用例结束时删除。

## 格式支持

哪些词典格式可被读取，以 `docs/DICTIONARIES.md` 的结论为准；本 crate 不解释词典内容。
