# 贡献指南

## 开发环境

- Windows 10/11 x64
- Rust（`x86_64-pc-windows-gnu` 工具链）+ MinGW-w64（用于链接）
- 无需 Visual Studio，无需 Windows SDK

```bat
set PATH=<rust-toolchain>\bin;<mingw>\bin;%PATH%
cargo test --release --manifest-path dict-core\Cargo.toml
cargo test --release --manifest-path dict-input\Cargo.toml
cargo test --release --manifest-path dict-popup\Cargo.toml
```

## 代码约定

- **不允许**在生产路径里 `unwrap`/`expect`/`panic`（启动失败除外，且必须弹窗告知）
- 每个 `unsafe` 块必须有 `// SAFETY:` 注释说明前置条件
- 用户可见文案用中文；错误必须对用户可见（卡片 / 设置页 / 托盘气泡），不能只写日志
- **日志绝不记录选中的文字内容**，只记录长度与命中词典
- 修改接口前先更新 `docs/DESIGN.md`

## 提交 PR

1. Fork 并新建分支
2. 保持提交信息聚焦：一句话说明"改了什么 + 为什么"
3. 确保三个 crate 的测试都通过
4. 涉及 UI 的改动请附截图

## 报告 bug

请用 issue 模板，并尽量附上：

- 复现步骤（在哪个程序里取词、选中了什么）
- `%APPDATA%\dict-popup\dict-popup.log` 的相关片段（日志不含选中文字）
- Windows 版本、是否多屏、DPI 缩放

## 安全

见 [SECURITY.md](SECURITY.md)。请勿在公开 issue 中粘贴敏感内容。
