# 词典格式与免费词典调研

> 调研日期 2026-10-27。对象是 `dict-popup`：MIT、单 exe、离线，当前只读 StarDict
> （`.ifo` + `.idx` + **未压缩** `.dict`，见 [stardict.rs](../dict-core/src/stardict.rs)）。
> 每节固定回答五问：是什么 / 怎么分发 / 许可 / 解析难度 / 是否适配现有 reader。
> 数字只写来源明确给出的；来源没写的写「未标注」；我自己量的标「本机实测」。

## 结论摘要

排序按 **契合度 × 许可清晰度 × 解析成本**，不是按词条数。

| # | 候选 | 语言对 | 许可 | 许可清晰度 | 解析成本 | 与现有 reader 的关系 | 结论 |
|---|---|---|---|---|---|---|---|
| 1 | **FreeDict / WikDict 的 StarDict 导出** | 多语，含 `eng-zho` | CC BY-SA 3.0（逐词典不同） | 高（每个包内 `COPYING`） | 低：补 `.idx.gz` + `.dict.dz` | 同一格式，只缺压缩容器 | **P0，先做** |
| 2 | **CC-CEDICT** | 汉↔英 | CC BY-SA 4.0 | 高 | 极低：一行一条纯文本 | 需要新 reader（约 100 行） | **P1** |
| 3 | **ECDICT** | 英→汉 | MIT | 高（仓库 LICENSE），上游血统略杂 | 极低：CSV / 已有 StarDict 包 | 已在用 | 保留为默认英汉词典 |
| 4 | **Open English WordNet (OEWN)** | 英→英 | CC BY 4.0 | 高 | 中：JSON / WNDB | 新 reader | **P2（英英）** |
| 5 | Princeton **WordNet 3.0 / 3.1** | 英→英 | 自有许可（可再分发，须保留声明） | 高 | 中高：自有二进制/行式库 | 新 reader | 用 OEWN 代替；3.0 仅作兼容 |
| 6 | **kaikki.org / Wiktextract** | 数百语种 | CC BY-SA + GFDL | 高 | 中：JSONL 流式 | 新 reader，体积劝退 | 只做「可选导入」，不预置 |
| 7 | **GCIDE** | 英→英（1913） | GPL-2.0-or-later | 高 | 中：SGML 风标签 | 新 reader | **不建议**（GPL + 语料陈旧） |
| 8 | **MDX / MDict** | 取决于内容 | 格式无授权可言，内容风险高 | **低** | 高：块压缩 + 多编码 + HTML | 全新 reader | **不做**（见下） |
| 9 | **ABBYY Lingvo DSL** | 取决于内容 | 格式无正式声明；自由内容稀缺 | 低-中 | 中：codepage + 标签 | 新 reader | 不做 |
| 10 | **Babylon BGL** | 取决于内容 | 无公开规范；几乎无自由内容 | **低** | 高（逆向） | 新 reader | 不做 |
| 11 | **Apertium** | MT 语言对 | GPL | 高 | 中：`.dix` XML | 不适合 | 不做（不是释义词典） |

一句话：**先把 StarDict 这条已经打通的路补全（`.idx.gz` / `.dict.dz`），收益远大于新增任何一个格式**；
真正需要新 reader 的只有纯文本 CEDICT 和 JSON 形态的 WordNet。

## 逐项调研

### 1. ECDICT（skywind3000）

1. **是什么**：英汉双解词库，带考试标签（zk/gk/cet4/cet6/ky/toefl/ielts/gre）、BNC 与当代语料库词频、
   动词变位（`exchange` 字段）。仓库 README 称 `ecdict.csv` 为基础版 **76 万词条**，
   release 28 的《简明英汉字典增强版》称收词 **340 万**（[README](https://github.com/skywind3000/ECDICT)、
   [releases](https://github.com/skywind3000/ECDICT/releases)）。更新活跃：仓库 2026-10 仍有提交。
2. **分发**：仓库内 `ecdict.csv`（本机实测 GitHub API：**66,009,548 字节**）、`stardict.7z`
   （**51,577,449 字节**，内含 SQLite 而非 StarDict 文件）、`lemma.en.txt`；
   release 1.0.28（2017-09-20 发布，资产 2018 年更新）另提供 `ecdict-stardict-28.zip`（70,415,834 字节）、
   `ecdict-mdx-28.zip`（97,755,340 字节）、`ecdict-sqlite-28.zip`（216,765,132 字节）。
   ⚠️ 二进制包停更在 2018，CSV 才是活的主线。
3. **许可**：MIT（`Copyright (c) 2025 Linwei`，[LICENSE](https://github.com/skywind3000/ECDICT/blob/master/LICENSE)）。
   离线再分发、商用、改作都允许。**但**数据是拼装的（README 的 HISTORY 自己写明用了 `cdict`、
   WordNet、BNC 词频等），个别上游的许可没有逐条追溯记录；按 MIT 使用风险低，但不宜声称「全部条目权属干净」。
4. **解析难度**：CSV，UTF-8，`definition`/`translation` 是**多行**（字段内换行，不是 HTML）；
   用成熟 crate（`csv`）即可。StarDict 包则是成品，走现有 reader。
5. **适配**：已适配（当前默认词典）。真正的问题不在 ECDICT，而在**它打出来的 StarDict 包很可能是
   dictzip 压缩的 `.dict.dz`**——本机网络无法下载该 zip 验证，**未验证**。

### 2. CC-CEDICT（MDBG）

1. **是什么**：社区维护的汉英词典，繁简对照 + 拼音 + 英文释义。词条数官方页面未标注；
   **本机实测** 2026-10-26 抓取的 MDBG 导出版为 **61,620 条**，解压后 **4,986,108 字节**。
   近乎每日更新（`Last-Modified: 2026-10-26`）。
2. **分发**：`https://www.mdbg.net/chinese/export/cedict/cedict_1_0_ts_utf-8_mdbg.txt.gz`
   （gz **3,982,067 字节**）；官网另有 zip 与「version 2」beta 版（[下载页](https://cc-cedict.org/editor/editor.php?handler=Download)）。
3. **许可**：CC BY-SA 4.0（[cc-cedict.org](https://cc-cedict.org/wiki/start) 与下载页均写明）。
   可离线分发、可商用，条件是**署名 + 相同方式共享**：我们改过的词典文件必须仍以 CC BY-SA 发布，
   并在应用「关于」里给出来源与许可链接。这不传染 MIT 代码，但传染「词典文件」。
4. **解析难度**：极低。UTF-8 纯文本，一行一条：`繁體 简体 [pin1 yin1] /义项/义项/`，
   `#` 开头为注释，首行有 `#! time=<epoch>` 版本戳。
   **坑**：行序近似按拼音（本机抽样第 30000 行附近是 `nai3`），不是按汉字字节序，
   所以不能直接对文件做二分，需自建索引或全量入内存（61k 条，内存哈希表可控在十几 MB）。
5. **适配**：需新 reader，但这是全部候选里最便宜的：纯文本 + 单行 + 一条正则。

### 3. Wiktionary 抽取（kaikki.org / Wiktextract）

1. **是什么**：把 Wiktionary 的 wikitext 转成机器可读 JSON 的抽取项目，覆盖数百种语言，
   释义用英语。版本按 Wiktionary dump 日期标注（当前为 **2026-10-01** 的 enwiktionary）。
2. **分发**：[rawdata.html](https://kaikki.org/dictionary/rawdata.html)——英文版 JSONL **24.8 GB**
   （gz **2.9 GB**），另有按语言切分的页面；「通常至少每周更新一次」。
3. **许可**：与 Wiktionary 相同，**CC BY-SA + GFDL**（[kaikki.org](https://kaikki.org/dictionary/) 页脚）。
   同样有 share-alike 义务。
4. **解析难度**：JSONL，一行一个对象，UTF-8，嵌套深（senses/forms/translations），无索引：
   查词要么全量扫、要么离线转成自有格式；**体积与单 exe 6.5 MB 的定位直接冲突**。
5. **适配**：不内置；留作「用户自备 JSONL 导入」。

### 4. Princeton WordNet

1. **是什么**：英文词义网络。3.0 版规模（官方 `wnstats` 手册）：**155,287 个唯一词形 / 117,659 个 synset /
   206,941 条词义**（[wnstats(7WN)](https://man.freebsd.org/cgi/man.cgi?query=wnstats&sektion=7wn)）。
   3.1 版之后 Princeton 基本停更（此说法来自二手资料，未直接核实）；站点现在多处不可达（见「未核实」）。
2. **分发**：`https://wordnetcode.princeton.edu/3.0/WordNet-3.0.tar.bz2`（**9,060,524 字节**，2007-08-22）、
   `https://wordnetcode.princeton.edu/wn3.1.dict.tar.gz`（**16,358,468 字节**，2012-02-27）。
3. **许可**：Princeton 自有许可：允许使用/复制/修改/分发，「for any purpose and without fee or royalty」，
   条件是**保留版权声明与免责声明**，且不得用 Princeton 名义做推广
   （[license.txt 原文](https://sources.debian.org/src/wordnet/1%3A3.0-36/lib/wnres/license.txt/)）。
   → 可以随 MIT 应用分发，需附 NOTICE。
4. **解析难度**：中高。`data.noun` / `index.noun` 行式文本 + 字节偏移指针 + synset 关系，
   要用起来得实现 synset/pointer 模型；只要「词 → 英文释义」可以离线摊平。
   它是 8-bit 编码（重音字符按单字节处理，具体映射未逐字节核实），没有 UTF-8 可依赖。
5. **适配**：需新 reader；建议直接用 OEWN 的 JSON 导出，别自己写 WNDB 解析器。

### 5. Open English WordNet（OEWN，WordNet 的活跃分支）

1. **是什么**：Princeton WordNet 的开源分叉，按年发版；2025 版含 Namenet 专名。
   规模：核心版 **135,969 词 / 107,519 synset**，Plus 版 **161,875 词 / 120,564 synset**
   （[README](https://github.com/globalwordnet/english-wordnet)）。
2. **分发**：GitHub Release `2025-edition`（发布于 2025-12-31），资产 `english-wordnet-2025-json.zip`
   （**9,986,555 字节**）、`english-wordnet-2025.zip`（WNDB，9,618,697 字节）、`.xml.gz`（11,363,503 字节）；
   另有 `en-word.net` 的 JSON API 与 `wn` Python 库。
3. **许可**：**CC BY 4.0**（README「License」节）。署名即可，**没有 share-alike**——比 CC-CEDICT 更好相处。
4. **解析难度**：JSON 版是 `{id, synsets:[{definition, ili, ...}]}` 形态，UTF-8，无压缩陷阱；
   体积约 10 MB 压缩包，展开后在可接受范围（需实测）。
5. **适配**：需新 reader，但只读一种 JSON 结构；也可以让 OEWN 社区脚本导出 StarDict（其 `RELEASING.md`
   提到新增了 StarDict 导出脚本，本机未验证产物）。

### 6. GCIDE（GNU Collaborative International Dictionary of English）

1. **是什么**：1913 年 *Webster's Revised Unabridged* 的电子化 + 志愿者增补（含部分 WordNet 1.5 释义）。
   英英，语料古老（README 自己警告「definitions are in most cases over 100 years old」）。词条数**未标注**。
2. **分发**：`https://ftp.gnu.org/gnu/gcide/`，最新 **0.54（2024-12-31）**，`gcide-0.54.tar.gz` **18 MB** 量级
   （目录页实测；另有 `.tar.xz` 14 MB、`.zip` 18 MB，均带 GPG 签名）。
3. **许可**：**GPL-2.0-or-later**（[README.DIC](https://ftp.gnu.org/pub/old-gnu/gcide/gcide-0.46/README.DIC)）。
   → 与 MIT 应用一起分发是真实的法律复杂度：要么把它当独立数据包分发并附 GPL 全文与源码，
   要么不预置。对一个「单 exe、MIT、公开」的产品，这是我不建议引入的主要原因。
4. **解析难度**：中。26 个 `cide.*` 文本文件，SGML 风标签（`<hw> <pr> <pos> <def> <q>`），
   特殊字符用 `webfont.asc` 另作映射，README 明确警告 `<p>` 与 `<note>/<cs>` **嵌套不合法**。
5. **适配**：需新 reader，且对中英用户几乎无增量价值（纯英英、释义过时）。

### 7. MDict / MDX

1. **是什么**：MDict 客户端的词典容器。`key → record` 关联数组，`record` 通常是 HTML 片段；
   `.mdd` 是配套资源包。
2. **分发**：格式本身无官方下载；内容散落在各论坛/网盘。合法且明确的自由内容源很少，
   典型是 **ECDICT 自己打的 MDX**（release 28，`ecdict-mdx-28.zip`，MIT）。
3. **许可**：这里必须分成两件事——
   - **格式**：没有公开规范。`writemdict` 的格式文档开头就写「软件不是开源，格式也没有公开规范，
     以下描述基于逆向，可能不完整、不准确」（[fileformat.md](https://github.com/zhansliu/writemdict/blob/master/fileformat.md)）。
     格式本身不可版权，实现读取没有许可障碍，但要接受「规范是二手的」。
   - **内容**：**这是最大的法律风险点**。中文圈流通的 MDX（牛津高阶、朗文、柯林斯、韦氏……）
     绝大多数是未授权的商业词典转换，随 MIT 应用推荐下载等于替侵权背书。**这一点是我基于生态观察的判断，
     没有找到能逐条核实的权威统计**，因此请在文档/UI 里按「来路不明，不提供下载」处理。
4. **解析难度**：高。header 是 UTF-16LE 的 XML；正文编码可为 UTF-8/UTF-16/GBK/Big5；
   词条块与记录块分别 zlib/LZO/不压缩；`Encrypted` 位可能触发 Salsa20/8 加密；
   记录里是 HTML（还可能有内联 CSS、发音链接）。
5. **适配**：与「StarDict 形状」差得最远，等于再写一个完整 reader，还要处理加密与 HTML 清洗。

### 8. ABBYY Lingvo DSL

1. **是什么**：Lingvo 的**源文件**格式，纯文本、人可编辑，条目 = 词头 + 正文（正文里带 `[trn]`、`[ex]` 类标签）。
   语言对取决于词典，俄英为主，中英极少。词条数**未标注**（按词典而异）。
2. **分发**：格式没有官方站点；内容是各家 `.dsl` / `.dsl.dz` 包。GoldenDict-ng 把它列为受支持格式
   （[dictformats](https://xiaoyifang.github.io/goldendict-ng/dictformats/)）。
3. **许可**：格式由 ABBYY 在 Lingvo 帮助文档里公开描述（[DSL dictionary structure](https://documentation.help/ABBYY-Lingvo8/DSL_MAIN_DLG.htm)），
   但**我没有找到任何针对格式本身的许可声明**——所以「读这个格式合不合法」这一问，我的答案是
   「格式不受版权保护，实现无碍；内容许可取决于每个词典」。自由许可的 DSL 内容很少见（未系统核实）。
4. **解析难度**：中。UTF-16 或 ANSI，ANSI 时必须读 `#SOURCE_CODE_PAGE`（仅 1252/1251/1250 三种）；
   头部 `#NAME` / `#INDEX_LANGUAGE` / `#CONTENTS_LANGUAGE` 明确；正文标签与嵌套需要处理。
5. **适配**：需新 reader。对中英用户收益极低，不划算。

### 9. Babylon BGL

1. **是什么**：Babylon 的词典容器，key → 词条，含 HTML 与可选资源。
2. **分发**：Babylon 官方渠道已不再是主流；现存 `.BGL` 多是历史词典的光盘转换。
3. **许可**：**无公开规范**（GoldenDict-ng 的实现属于逆向；Archive Team 的 BGL 条目本机访问超时，未能核对原文）。
   内容方面**未找到任何以自由许可发布的 BGL 源**——流通品基本是商业词典转换，风险与 MDX 同级甚至更高。
4. **解析难度**：高（块索引 + 压缩 + 编码嗅探）。
5. **适配**：不做。收益近乎为零，风险最高。

### 10. FreeDict

1. **是什么**：2000 年起的自由双语词典集合，官网称 **140+ 词典 / 约 45 种语言**；
   我拉取 `freedict-database.json` 实测为 **306 条词典记录**（含多平台发布项）。
   中英对有两个：`eng-zho`（**26,660 词头**，2025-11-23 版，由 WikDict 从 Wiktionary/DBnary 生成）、
   `fra-zho`（10,947 词头）。
2. **分发**：`download.freedict.org` 按 `词典/版本/` 组织，每个词典有 `src`(TEI) / `dictd` / `slob` / `stardict`
   四种包；`eng-zho` 的 StarDict 包 **1,672,048 字节**，解包后是 `.ifo` + **`.idx.gz`** + **未压缩 `.dict`**（本机实测）。
   另有 JSON API：[freedict-database.json](https://freedict.org/freedict-database.json)（每个 release 带 `version`、`date`、`size`、`sha512`）。
3. **许可**：逐词典不同，必须看包内 `COPYING`。`eng-zho` 的 TEI 头写明
   **CC BY-SA 3.0 Unported**。→ 可离线分发，须署名 + 同许可。
4. **解析难度**：源是 TEI P5 XML（带 DTD/RNG）；对我们重要的是它的 StarDict 导出，**格式同现有 reader**。
   **坑**：`.idx` 是 gzip 过的——当前 reader 只认裸 `.idx`（[stardict.rs L90](../dict-core/src/stardict.rs#L90)），
   拿到手也打不开。
5. **适配**：**最契合**——补上 `.idx.gz` 就是零新增格式。

### 11. WikDict

1. **是什么**：从 Wiktionary（经 DBnary）生成的机器可读双语文档项目，语种对较多；规模按语言对**未标注**。
2. **分发**：[下载页](https://www.wikdict.com/page/download)：StarDict、Kobo、SQLite（原生）、TEI P5 四种。
3. **许可**：**CC BY-SA**（页面原话「use them according to the Creative Commons BY-SA license」）。
4. **解析难度**：StarDict 路线同上（大概率是 `.dict.dz`，未验证）；SQLite 路线要引入 SQLite 依赖，
   与「零运行时依赖、单 exe」冲突。
5. **适配**：走 StarDict 导出即可，与 FreeDict 同一套修复。

### 12. Apertium，以及其他汉语/中英来源

- **Apertium**：GPL 的规则机器翻译平台，词法数据是 `.dix` XML（lttoolbox monodix/bidix，[wiki](https://wiki.apertium.org/wiki/Monodix)）。
  它是**翻译系统的词法表**，不是带释义的词典；中英对支持也有限。**不做。**
- **CC-Canto**：粤英词典，**CC BY-SA 3.0**，数据格式与 CC-CEDICT 类似
  （[about](https://cantonese.org/about.html)）。将来做粤语可复用 CEDICT reader。
- **萌典 / 教育部辞典**：原始数据为 **CC BY-ND 3.0 台湾**授权（[about](https://www.moedict.tw/about.html)）。
  **ND 禁止改作**——不能重新排版、不能转成自有格式分发，对词典应用是硬伤，不做。
- **汉典（zdic.net）等站点**：内容版权保留，未见开放许可声明；不使用。

## 格式对比

| 维度 | StarDict | MDX | DSL | JSONL | 纯文本（CEDICT/CSV） |
|---|---|---|---|---|---|
| **文档质量** | **好**：StarDict 源码树里的 `dict/doc/StarDictFileFormat` 是权威规范，字段、字节序、排序规则全写明（[原文](https://github.com/huzheng001/stardict-3/blob/master/dict/doc/StarDictFileFormat)） | **差**：官方无规范，社区逆向文档自认「可能不完整、不准确」 | 中：ABBYY 帮助文档说明了头部与编码，正文标签集需另查 | 好（自行描述 schema） | 好（CEDICT 有语法页） |
| **编码坑** | 低：全 UTF-8，数字一律网络字节序 | **高**：header UTF-16LE，正文 UTF-8/UTF-16/GBK/Big5 四选一，另可选加密 | 中：UTF-16 或 ANSI + 3 种 codepage | 低：UTF-8 | 低：UTF-8（Excel 直接打开会乱码，程序无碍） |
| **查找需要** | 需要有**序** `.idx`（规范强制 `stardict_strcmp` 排序），二进制搜索；索引可全量 mmap | 需要块索引（`key_index` 记录每块首末词 + 压缩块偏移），本质是分块二分 | 需自己解析全文并建索引（无原生索引） | 无索引，要么流式全扫，要么先转自有格式 | 无索引；CEDICT 行序按拼音而非字节序，不能直接二分 |
| **压缩** | `.idx.gz`、`.dict.dz`（dictzip，带随机访问块表）；规范说「正常情况下应当用 `.idx` + `.dict.dz`」 | 每块独立 zlib / LZO / 不压缩 | `.dsl.dz`（整文件 gzip，**无随机访问**） | 整文件 gzip | 整文件 gzip |
| **更新故事** | 无中心仓库：靠 FreeDict / WikDict / ECDICT 各自发版；StarDict 官方站已名存实亡 | 无版本概念，靠文件名与社区 | 同左 | kaikki 每周；CEDICT 近乎每日；JSON 风格便于 diff | CEDICT 有 `#! time=` 版本戳；ECDICT 有 git 历史 |
| **现状** | 生态事实标准，工具链齐全（pyglossary 等） | 用户多、内容乱、规范二手 | 存量俄英词典多，新内容少 | 现代、可流式、体积大 | 最简单，适合小词典 |

**对本项目的直接结论**：`dict-core` 目前只处理 `.idx`（非 gz）与 `.dict`（非 dz），
且 `sametypesequence` 只对 `"m"` 走了快路径，其余类型按「类型字节 + NUL 分隔」解析
（[stardict.rs L324](../dict-core/src/stardict.rs#L324)）——这对 `tm`、`h` 这类序列是错的，
遇到真实词典会解析出乱码。这两个修复的优先级高于任何新格式。

## 建议路线

**P0（先做，1 个 reader 都不用加）：把 StarDict 补全。**
1. `.idx.gz`：gunzip 到内存或流式解压后建偏移表（`idxfilesize` 字段给的是原始大小，可直接预分配）。
2. `.dict.dz`：dictzip 是「gzip + 每块长度表（放在 gzip extra field）」，实现块级随机读即可，
   不需要解压整个文件；规范原文见上表链接。
3. 修 `sametypesequence`：按序列逐类型解析（小写类型读到 `\0`，大写类型先读 u32 长度），
   顺带支持 `.syn` 与 `idxoffsetbits=64`。收益：FreeDict / WikDict / 现成 StarDict 词典立刻可用，**零新增许可风险**。

**P1：CC-CEDICT（汉→英）。** 61,620 条、4.9 MB、单行纯文本、CC BY-SA 4.0、近乎每日更新。
解析器可以写在一个文件里；索引用内存哈希表（繁/简/拼音三套 key）。这是当前产品**最大的功能缺口**
（现在只有英→汉）。

**P2：OEWN（英→英）。** CC BY 4.0 没有 share-alike，10 MB JSON，补上「英文释义 + 同义/反义」，
正好补 ECDICT 中文释义之外的短板。WordNet 3.0 用 OEWN 顶替，避免自写 WNDB 解析器。

**P3（可选）：通用 JSONL 导入。** 给 kaikki 子集或用户自有数据留口子，但不预置、不内置下载。

**明确不做的：**

| 不做 | 理由 |
|---|---|
| **MDX** | 格式无权威规范（逆向文档自认不准确）+ 加密分支 + HTML 清洗；更要命的是**内容**：流通的几乎都是未授权商业词典，公开推荐等于引火烧身。收益（用户多）抵不过风险。 |
| **BGL** | 格式逆向、官方已边缘化、几乎不存在自由许可内容。 |
| **DSL** | codepage 麻烦、自由内容稀缺、对中英用户零增益。 |
| **GCIDE** | GPL-2.0-or-later 与 MIT 单 exe 分发有明显摩擦（聚合分发能否成立可争论，但风险落在项目方）；且是 1913 年英文语料，产品价值低。 |
| **Apertium** | 是 MT 词法数据，不是词典。 |
| **萌典** | CC BY-ND：禁止改作，无法转格式分发。 |

## 下载与版本

| 源 | 稳定 URL / API | 可 pin？ | 典型体积 | 客户端如何发现新版本 |
|---|---|---|---|---|
| ECDICT (CSV) | `https://raw.githubusercontent.com/skywind3000/ECDICT/master/ecdict.csv` | 是：按 commit SHA 取 raw URL | 66 MB | GitHub API `commits?path=ecdict.csv` 的 sha/date |
| ECDICT (发布包) | GitHub Releases `1.0.28` 资产 | 是 | StarDict 包 70.4 MB；MDX 包 97.8 MB | **不建议**：资产停在 2018，永远「没有新版」 |
| CC-CEDICT | `https://www.mdbg.net/chinese/export/cedict/cedict_1_0_ts_utf-8_mdbg.txt.gz` | 否（滚动覆盖） | gz 3.98 MB / 解压 4.99 MB | ① 文件首行 `#! time=<epoch>`；② HTTP `Last-Modified`；③ 本地保存上次 epoch 比对 |
| FreeDict | `https://freedict.org/freedict-database.json` + `https://download.freedict.org/dictionaries/<pair>/<edition>/...` | **是**：URL 内含 `edition` | `eng-zho` StarDict 包 1.67 MB / 解包 dict 8.1 MB | 拉 JSON，比 `edition`（如 `2025.11.23`）与每个 release 的 `sha512` |
| WikDict | `https://www.wikdict.com/page/download` 的 StarDict 链接 | 部分（按语言对目录） | 未标注 | 无版本 API，只能比对文件 hash/日期 |
| OEWN | GitHub Releases `2025-edition` | **是**：tag + 资产名 | JSON zip 9.99 MB；WNDB zip 9.62 MB；xml.gz 11.36 MB | GitHub Releases API 的 `tag_name` / `published_at` |
| Princeton WordNet | `https://wordnetcode.princeton.edu/wn3.1.dict.tar.gz` | 是（文件名含版本，内容 2012 年起未变） | 16.36 MB | 不需要：实质已冻结 |
| GCIDE | `https://ftp.gnu.org/gnu/gcide/gcide-latest.tar.gz` | 是（另有 `gcide-0.54` 定版） | 18 MB（gz）/ 14 MB（xz） | `gcide-latest` 的 `Last-Modified`，或读 `README.DIC` 里的版本号 |
| kaikki | `https://kaikki.org/dictionary/rawdata.html` | 弱（按 dump 日期） | 24.8 GB JSONL / 2.9 GB gz | 页面标注的 dump 日期（当前 2026-10-01）；只对「导入」有意义 |

**许可落地要求（不是可选项）**：CC BY-SA / CC BY 都要求署名与许可全文。
建议在 `设置 → 关于` 列出「词典名 / 来源 URL / 许可 / 版本日期」，并在安装目录放 `THIRD-PARTY-NOTICES.md`；
用户替换词典文件时，同一份声明要跟着走。

## 未核实 / 不确定

1. **Princeton WordNet 站点**：`wordnet.princeton.edu`（含 `/license-and-commercial-use`）本机多次 404/超时，
   许可原文改用了 Debian 源码包里的 `license.txt`（与 SPDX 记录一致）；站点是否已迁移，**未核实**。
2. **ECDICT 的 StarDict 包是否用 dictzip**：GitHub release 资产本机下载失败（`curl` exit 56），
   所以是 `.dict` 还是 `.dict.dz` **未验证**。这决定 P0 里 dictzip 的优先级。
3. **MDX 生态的侵权比例**：属于判断，没有可引用的统计。文中已按「来路不明」处理。
4. **BGL 的规范状态**：Archive Team 的 BGL 条目本机超时，没读到原文；只确认了 GoldenDict-ng 支持该格式，
   以及 `stardict.org` 只剩一句「Click here to enter」、`download.huzheng.org` 无响应
   （规范文档因此改引 StarDict 源码树里的 `dict/doc/StarDictFileFormat`，2020-11-28 修订，Hu Zheng）。
5. **OEWN 的 StarDict 导出**：`RELEASING.md` 提到新增了 StarDict 导出脚本，产物是否随 release 发布**未验证**。
6. **各词典的条目数**：ECDICT 76 万/340 万、OEWN 135,969/161,875、WordNet 155,287、FreeDict 26,660
   均为来源自述；CC-CEDICT 的 61,620 是我本地统计的，官方页面未给数字。
