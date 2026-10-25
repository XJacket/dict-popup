# 安装为 UIAccess 应用

> 适用：Windows 11，dict-popup 0.2.x。UIAccess 只在**专门构建并签名**的版本上可用；
> 未签名的普通 release 不带 `uiAccess="true"`，不需要也不能照本文安装。

## 为什么需要它

Windows 的 UIPI（User Interface Privilege Isolation）规定：一个进程只能向**同级或更低**
完整性级别（Integrity Level）的窗口发送窗口消息和合成输入。

`dict-popup` 取词时要模拟一次 `Ctrl+C`。当目标窗口属于**以管理员身份运行**的程序
（高完整性）时，普通权限的 `dict-popup` 发过去的按键会被 UIPI 静默丢弃：热键有反应，
取词却超时或提示「输入被阻止」。

UIAccess 是 Windows 给辅助技术（屏幕阅读器、屏幕键盘等）提供的正式机制：进程 token
带上 UIAccess 标志后，即使自身以普通权限运行，也能和更高完整性的前台窗口交互。
它不是提权：进程权限没有变高，只是被允许驱动其他程序的 UI。边界见文末[安全边界](#安全边界)。

## 四个硬条件

缺一不可：

1. **manifest 带 `uiAccess="true"`**
   exe 内嵌的 manifest 必须包含：

   ```xml
   <requestedExecutionLevel level="asInvoker" uiAccess="true" />
   ```

   普通构建是 `uiAccess="false"`。UIAccess 应用以调用者权限（asInvoker）运行，不应要求管理员。

2. **Authenticode 签名有效，且证书被本机信任**
   `Get-AuthenticodeSignature` 必须是 `Valid`。关键是**机器级**信任：证书链的根要在
   `LocalMachine\Root`（自签证书就是证书本身）。只导入 `CurrentUser\Root` 不够，
   因为 UIAccess 的校验发生在系统服务里。

3. **exe 位于安全位置（`%ProgramFiles%`）**
   默认安装到 `%ProgramFiles%\dict-popup\dict-popup.exe`。放在桌面、下载目录或用户目录的
   exe 不会获得 UIAccess。快捷方式可以放在用户目录，但它指向的 exe 必须在受保护位置。

4. **系统策略允许**
   - UAC 必须开启；
   - 「用户帐户控制：只允许提升已签名的 UIAccess 应用程序安装在安全位置」必须为**已启用**
     （Windows 默认值）。配置位置：`secpol.msc` → 本地策略 → 安全选项，或 `gpedit.msc`
     → 计算机配置 → Windows 设置 → 安全设置 → 本地策略 → 安全选项。

## 为什么未签名时不能带这个 manifest

Windows 在创建进程时检查 manifest。一旦看到 `uiAccess="true"`，它要求 exe 有有效的
Authenticode 签名，并满足位置和策略条件。未签名或签名无效时 `CreateProcess` 直接失败，
exe **根本不会启动**（常见现象是双击无反应，或系统事件日志中出现创建进程失败）；
签名有效但位置/策略不满足时，程序通常还能启动，只是拿不到 UIAccess token。

因此项目有两个构建变体：

| 构建命令 | manifest | 用途 |
|---|---|---|
| `build.bat` | `uiAccess="false"` | 普通 release，未签名也能运行 |
| `build.bat /uiaccess` | `uiAccess="true"` | 只能签名后安装到 `%ProgramFiles%`；未签名不可启动 |

不要给普通构建手工改 manifest 后分发；也不要把未签名的 `/uiaccess` 产物发给别人。
签名必须在**构建之后**做，因为重新链接会覆盖签名。

## 安装步骤

1. **构建 UIAccess 变体**（普通终端即可）：

   ```bat
   build.bat /uiaccess
   ```

   产物仍是 `dist\dict-popup.exe`，但 manifest 已改为 `uiAccess="true"`。

2. **签名**。用本机信任的代码签名证书；没有证书时可以先跑仓库的自签脚本（仅本机可信）：

   ```powershell
   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\sign-release.ps1
   ```

   它会把证书装进 **CurrentUser** 的 Root/TrustedPublisher，而 UIAccess 需要机器级信任，
   所以再以管理员身份把导出的 `.cer` 导入机器根：

   ```powershell
   # 管理员 PowerShell；文件名以 sign-release.ps1 的输出为准
   Import-Certificate -FilePath .\assets\signing\dict-popup-codesign.cer -CertStoreLocation Cert:\LocalMachine\Root
   ```

   正式分发应使用公共 CA 或企业 CA 的代码签名证书，并加时间戳；自签证书只在导入过它的
   机器上有效。签名细节见 [SIGNING.md](SIGNING.md)。确认签名：

   ```powershell
   Get-AuthenticodeSignature .\dist\dict-popup.exe | Format-List Status, SignerCertificate
   ```

   期望 `Status : Valid`。

3. **安装**（管理员 PowerShell）：

   ```powershell
   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-uiaaccess.ps1
   ```

   默认使用 `dist\dict-popup.exe`，安装到 `%ProgramFiles%\dict-popup`，并创建
   `%ProgramData%\Microsoft\Windows\Start Menu\Programs\dict-popup.lnk`
   （快捷方式的工作目录已设为安装目录）。

   脚本会先检查 manifest 和签名，拒绝未签名或普通构建，也会拒绝 `%ProgramFiles%` 之外的目录；
   安装前会停掉正在运行的 `dict-popup`，并可重复运行。可用 `-Exe` 换用别的已签名产物，
   `-InstallDir` 指定 Program Files 下的子目录。

4. **从开始菜单启动，不要提权**：
   正常点 `dict-popup`。不要右键「以管理员身份运行」——UIAccess 的意义就是在普通权限下
   获得跨完整性访问；提权启动会改变完整性级别，日志里的 `elevated` 也和本文的验证路径不一致。

卸载（保留配置）：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-uiaaccess.ps1 -Uninstall
```

同时删除当前账户的 `%APPDATA%\dict-popup`（`config.json`、备份和日志）：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-uiaaccess.ps1 -Uninstall -PurgeConfig
```

## 怎么验证成功

前两条是自检，第三条才是决定性的。

1. **日志**

   ```powershell
   Select-String -Path "$env:APPDATA\dict-popup\dict-popup.log" -Pattern 'privilege:'
   ```

   最新一行应包含：

   ```text
   privilege: uiaccess=true ...
   ```

   如果日志不在 `%APPDATA%\dict-popup`，程序会依次回退到 exe 所在目录和 `%TEMP%`；
   启动日志里有一行 `log file:` 指出实际路径。

2. **设置 → 关于**
   应显示 `UIAccess: 已启用`（完整行类似 `UIAccess: 已启用 · 完整性: Medium`）。
   若显示「未启用」，说明没有拿到 token，按下一节排查。

3. **真实测试（必做）**
   1. 打开记事本：开始菜单搜索「记事本」→ 右键 → **以管理员身份运行**；
   2. 在**这个提权记事本**里输入或选中英文，例如 `hello`；
   3. 按 `Ctrl+Alt+D`。

   预期：鼠标旁弹出释义卡片。

   普通记事本里能取词**不能**证明 UIAccess 生效——那本来就不需要 UIAccess。
   只有在管理员窗口里也能取词，才说明跨完整性级别的合成输入被放行。

## 失败排查表

| 现象 | 可能原因 | 处理 |
|---|---|---|
| 双击或开始菜单启动无反应 | exe 带 `uiAccess="true"` 但未签名或签名无效 | 重新签名，`Get-AuthenticodeSignature` 必须为 `Valid`；安装脚本也会拒绝 |
| 启动失败，事件日志中有创建进程失败 | 证书链不被本机信任 | 把根证书导入 `LocalMachine\Root` |
| 能启动，但日志是 `privilege: uiaccess=false` | 装成了普通构建（`uiAccess="false"`） | 用 `build.bat /uiaccess` 重新构建、签名、重装；不要混用 `dist` 里的旧文件 |
| 同上 | exe 不在 `%ProgramFiles%`（被复制到桌面或用户目录） | 用安装脚本装到默认目录；不要单独复制 exe 运行 |
| 同上 | 证书只在 `CurrentUser\Root`，不在 `LocalMachine\Root` | 管理员执行 `Import-Certificate ... Cert:\LocalMachine\Root`，完全退出程序再启动 |
| 同上 | UAC 关闭，或「只允许提升已签名的 UIAccess 应用程序安装在安全位置」被禁用 | 开启 UAC 和该策略（默认已启用），重启程序；必要时重启系统 |
| 安装脚本提示 `does not contain uiAccess="true"` | 用了普通构建 | 换 `/uiaccess` 构建的 exe |
| 安装脚本提示 unsigned 或 signature not valid | 未签名、签名损坏或证书不受信任 | 先完成签名，再安装 |
| 安装脚本提示需要管理员 | 当前终端未提权 | 用「以管理员身份运行」打开 PowerShell |
| 安装脚本拒绝目录 | `-InstallDir` 不在 `%ProgramFiles%` 下 | 用默认值，或选 Program Files 的子目录 |

改完证书或策略后，先**完全退出** `dict-popup`（托盘退出）再启动。UIAccess 在进程创建时授予，
运行中不会变化。

验证时不要用「以管理员身份运行」：高完整性进程本来就能向管理员窗口发送输入，会让
「是否拿到 UIAccess」无法判断。

## 安全边界

- **UIAccess 能驱动其他程序的 UI**：可以向更高完整性的窗口发送合成输入、读取 UI 信息，
  也能与提权程序交互。它不提升进程权限，但足以代表用户操作别的程序。只应安装自己构建或
  明确信任的已签名版本；不要给来源不明的 exe 签名或安装。
- **安全桌面不可用**：UAC 同意框和登录/锁屏界面运行在安全桌面上，UIAccess 程序不能与
  它们交互。所以本程序无法自动确认 UAC，也不能在锁屏界面取词。
- **剪贴板路径**：程序读写剪贴板本身不需要 UIAccess（剪贴板 API 可跨完整性级别使用）；
  需要 UIAccess 的是「向更高完整性窗口发送 Ctrl+C」这一步。没有 UIAccess 时，普通窗口里
  的取词仍然可用，只是提权窗口里会失败。
- 签名只证明「这个 exe 由该证书的持有者签发且未被篡改」，不证明代码没有缺陷；是否信任
  某个签名证书由你决定。

## 参考

- [User Account Control: Only elevate UIAccess applications that are installed in secure locations](https://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-10/security/threat-protection/security-policy-settings/user-account-control-only-elevate-uiaccess-applications-that-are-installed-in-secure-locations)
- [Policy CSP - LocalPoliciesSecurityOptions](https://learn.microsoft.com/en-us/windows/client-management/mdm/policy-csp-localpoliciessecurityoptions)
