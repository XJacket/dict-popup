# 任务：获取代码签名证书（SignPath Foundation）

- **Type:** task
- **Status:** **blocked**（2026-10-26，用户当前无法申请）
- **Started:** 2026-10-25
- **Owner:** 用户（提交申请）+ agent（跟进、准备、验证）

## 为什么 blocked

用户确认**暂时无法为该项目申请到证书**。申请通道本身没有技术阻塞，缺的是能提交/通过审核的条件，
不在 agent 能推进的范围内。

**替代方案已落地并在本机验证**：自签名 + UIAccess，流程写在
[README 的「本地自签名安装」](../../README.md#本地自签名安装启用-uiaccess)，
证据见 [docs/adr/0001](../../docs/adr/0001-uiaaccess-for-cross-integrity-capture.md)。
"能跨完整性级别取词"这件事**不依赖 SignPath**，只依赖本机自签。

**恢复条件**：用户重新具备申请条件时，把 Status 改回 `in-progress`；
申请材料见 `docs/SIGNPATH-APPLICATION.md`。

## 目标

让 `dict-popup` 的 release 具备被 Windows 全平台信任的 Authenticode 签名。

## 为什么阻塞

用户实测：以管理员身份运行一切正常，普通权限下取词失败、且系统提示"未知发布者"。
成因指向进程完整性级别与系统对未签名程序的限制——签名是当前最可能的解法，但**尚未被验证**。
因此本任务完成的标准不是"申请通过"，而是：**签名后的 release 在普通权限下能取词**。

## 状态

| 步骤 | 状态 | 备注 |
|---|---|---|
| 仓库公开 + MIT | ✅ 完成 | https://github.com/XJacket/dict-popup |
| 首个公开版本 | ✅ 完成 | `v0.2.0`（pre-release，未签名），SHA-256 记录在 release notes |
| CI 从源码构建产物 | ✅ 完成 | `.github/workflows/ci.yml` |
| Release 工作流预留签名步骤 | ✅ 完成 | `.github/workflows/release.yml`，`if: false` |
| 申请材料 | ✅ 完成 | `docs/SIGNPATH-APPLICATION.md` |
| **提交 SignPath 申请** | ⬜ **待用户** | 需注册账号 + 网页表单；agent 无法代做 |
| 获取 org id / project slug / policy slug | ⬜ 待批准 | SignPath 后台给出 |
| 配置仓库 secrets | ⬜ 待批准 | `SIGNPATH_API_TOKEN`、`SIGNPATH_ORGANIZATION_ID` |
| 开启 workflow 签名步骤 | ⬜ 待批准 | 两个 `if: false` → `if: true` |
| 签名后验证普通权限取词 | ✅ **机制已验证**（2026-10-25，自签） | `uiaccess=true elevated=false integrity=High`，提权记事本取词成功；等 SignPath 正式签名复验 |

最后检查：**2026-10-26 10:46**（automation task）——材料 `docs/SIGNPATH-APPLICATION.md` 与仓库现状一致：
仓库 `XJacket/dict-popup`、MIT、`cmd /c build.bat` 产 `dist/dict-popup.exe`、工作流
`.github/workflows/release.yml`、已有 release `v0.2.0`、HEAD `d28eb86`。当前阻塞点仍是
**用户提交申请**。

## 每 12 小时应当做的事

1. 读本文件，确认上表哪一步是当前阻塞点。
2. 若"提交申请"仍未完成：提醒用户提交，并确认 `docs/SIGNPATH-APPLICATION.md` 内容仍与仓库一致。
3. 若已提交、等待审核：检查用户是否已有回复（问一句即可，不要重复打扰）。
4. 若已批准：按顺序配置 secrets、开启 workflow、打一个 `v0.2.1` 验证签名，然后跑
   "普通权限启动 + 记事本取词"的验证，把结果写回本文件。
5. 每次唤醒只推进一步，更新本文件的状态表，并在回复中给出**一句话状态 + 下一步需要谁**。

## 若不通过的退路

- 因"使用量不足"被拒：先积累 star / issue / release 下载量，30 天后重新申请；
  签名流水线已就绪，不需要改代码。
- 若签名后普通权限**仍然**取不到词：说明成因是完整性级别而非签名信任，
  回到 `#02` 继续排查（改用 UIA 全文比对、或以计划任务/启动器提升权限的替代方案）。
