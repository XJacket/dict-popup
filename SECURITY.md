# 安全策略

## 报告漏洞

请**不要**开公开 issue。用 GitHub 的 [Private vulnerability reporting](../../security/advisories/new)
或直接邮件联系维护者（见仓库 profile）。

我们会在 72 小时内回应。

## 程序处理的数据

`dict-popup` 完全离线运行。需要你知道的几点：

| 数据 | 处理方式 |
|---|---|
| 选中的文字 | 只在内存中用于查询；**不会**写入日志、不会发送到网络 |
| 剪贴板 | 取词时短暂备份并还原（文本与图片）；若还原失败，原内容可能丢失 |
| 配置 | `%APPDATA%\dict-popup\config.json`，纯本地 |
| 日志 | `dict-popup.log`，只含长度、词典名、错误原因，不含选中文字 |

程序不包含任何网络请求代码，也不读取词典以外的文件。
