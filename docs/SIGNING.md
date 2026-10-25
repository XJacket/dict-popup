# 代码签名

## 现状

Release 二进制**未使用公共 CA 证书签名**。从源码构建或下载未签名 release 时，
Windows 会提示"未知发布者 / 此应用已被阻止"，这是未签名开源软件的正常现象，
可在提示中点「更多信息 → 仍要运行」。

> 注意：如果 exe 被放在**低完整性**目录（某些沙箱、受限目录）里，Windows 不仅会提示
> 未签名，还会阻止程序向其他程序发送按键（UIPI），导致取词失败。把程序放到普通目录
> 即可。程序启动时会自检完整性级别并在设置页提示。

## 计划：SignPath Foundation 免费开源签名

[SignPath Foundation](https://signpath.org/) 为符合条件的开源项目提供免费的
代码签名（以 SignPath Foundation 名义签发，Windows 全平台信任）。
这是目前唯一无需付费即可获得"被所有 Windows 信任"签名的途径。

### 申请条件（据其公开说明）

1. 项目**公开开源**，有 OSI 认可的许可证
2. 通过 **CI 构建**产物（GitHub Actions / Azure DevOps）
3. 项目有一定使用量或社区活跃度
4. 签名只用于该项目的官方 release

### 申请时用到的信息

| 项 | 值 |
|---|---|
| 仓库 | `https://github.com/XJacket/dict-popup` |
| 许可证 | MIT |
| 产物 | `dist/dict-popup.exe`（单文件，Windows x64） |
| 构建命令 | `build.bat`（内部：`cargo build --release` + `windres` 资源链接） |
| CI 工作流 | `.github/workflows/release.yml` |
| 发布方式 | GitHub Releases（zip） |
| 项目简介 | 离线划词词典：选中文字按热键，弹出释义卡片；纯本地 StarDict 词典，无网络 |

### 通过后的流程

1. SignPath 会给出 organization id、project slug、signing policy slug
2. 把这些值写进仓库 Secrets：`SIGNPATH_API_TOKEN`、`SIGNPATH_ORGANIZATION_ID`
3. `.github/workflows/release.yml` 里已预留签名步骤（见该文件注释）
4. 之后每次打 tag，CI 构建 → 提交签名 → 下载已签名产物 → 生成 release

## 自行签名（本机可信）

个人自用可以自签，Windows 只在**安装过该证书的机器**上信任：

```powershell
# 生成证书 + 装入当前用户的 Root/TrustedPublisher + 签名
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\sign-release.ps1
```

自签证书不会改变 SmartScreen 在其他机器上的提示——这是证书信任链的属性，不是程序缺陷。
