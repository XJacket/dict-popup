# SignPath Foundation 申请材料

> 用途：向 [SignPath Foundation](https://signpath.org/) 申请免费的开源代码签名。
> 申请通过后，release 产物将由 SignPath Foundation 签名，Windows 全平台不再提示
> "未知发布者"。
>
> 申请入口：<https://signpath.org/> → *Apply for free signing* →
> *SignPath Foundation open-source application* 表单。

## 逐项填写内容（可直接复制）

**Project name**

```
dict-popup
```

**Repository URL**

```
https://github.com/XJacket/dict-popup
```

**License**

```
MIT
```

**Project description（英文，建议原文使用）**

```
dict-popup is a small, fully offline dictionary popup for Windows. The user
selects a word in any application and presses a global hotkey; a frameless
floating card shows phonetics and definitions from a local StarDict dictionary
(e.g. ECDICT, 3.4 million entries). The application makes no network requests,
ships as a single 6.5 MB executable with no runtime dependencies, and is written
in Rust (three crates: dictionary reader, Win32 input layer, egui UI).
```

**What is being signed**

```
One Windows x64 executable: dict-popup.exe, distributed inside a zip archive
in GitHub Releases. No installers, no DLLs, no background services.
```

**Build instructions（用于可复现构建）**

```
GitHub Actions, windows-latest:
  cmd /c build.bat            # cargo build --release + windres resource linking
  powershell scripts/package.ps1 -SkipBuild
Artifact: dist/dict-popup.exe
Workflow: .github/workflows/release.yml
```

**CI system**

```
GitHub Actions
```

**Why code signing is needed**

```
Windows SmartScreen flags the unsigned executable as coming from an unknown
publisher, which deters users from an otherwise fully offline utility. A
SignPath Foundation signature lets users verify that the binary was built from
this public repository's CI.
```

**Contact / maintainer**

```
GitHub: @XJacket
```

## 申请通过之后要做的三件事

1. **拿到三个 slug**（SignPath 后台 → Project → Signing policy / Artifact configuration）
2. **写进仓库 Secrets**：
   - `SIGNPATH_API_TOKEN`（SignPath 生成的 CI token）
   - `SIGNPATH_ORGANIZATION_ID`
3. **改 `.github/workflows/release.yml`**：
   - 填 `SIGNPATH_ORGANIZATION_ID`
   - 把两个 `if: false` 改成 `if: true`
   - 打 tag（`v0.1.0`）触发，签名后的 exe 会自动替换并打包发布

## 常见被拒原因（自查）

| 原因 | 我们的情况 |
|---|---|
| 项目不开源 | ✅ 公开仓库、MIT |
| 无 CI 构建 | ✅ `.github/workflows/release.yml` |
| 产物不是 CI 构建的 | ✅ release 工作流从源码构建 |
| 项目无人使用 | ⚠️ 新项目，可能被要求"有一些用户后再申请" |
| 签名用于第三方/闭源组件 | ✅ 仅签本项目产物 |

> 如果因"使用量不足"被拒：先把仓库发布、写 README、发一个 release，积累 star 与 issue
> 后重新申请即可，签名流程已经准备好，不需要改代码。
