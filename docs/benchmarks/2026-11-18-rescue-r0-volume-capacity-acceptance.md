# UCode Rescue R0 单卷容量诊断正式验收报告

日期：2026-11-18
平台：Windows 11 x64
能力：`storage.volume.inspect@1`
风险等级：R0 / 只读

## 验收结论

UCode 已实现针对一个用户明确指定的本地 Windows 盘符的真实容量诊断。Rust/Tauri 宿主直接调用 Windows API，返回总容量、调用者可用容量、已用容量、使用率、卷类型和只读标志。模型不生成查询命令，探针不启动 PowerShell、CMD、WMI 或外部进程。

真实桌面验收确认：当前 `C:` 只剩约 3.5 GiB、已用 98.2%，UCode 将其标记为严重容量压力；不存在的 `Z:` 被保留为 unavailable，没有误报为空间充足。两轮均没有联网、审批卡、文件枚举、删除或系统修改。

## 安全与隐私边界

- 一次只接受一个明确 ASCII 盘符；
- 拒绝 UNC、网络盘、卷 GUID、目录路径、多个盘符、通配符、环境变量和命令拼接；
- 不枚举其他卷；
- 不返回卷标、序列号、文件系统名称、用户名、文件名或目录；
- 不读取文件内容；
- 不清理缓存、回收站、临时目录或更新文件；
- 不执行 CHKDSK、SMART、坏道扫描或文件系统修复；
- 即使用户写了“清理”，本轮仍只诊断并明确没有删除任何文件。

## 原生实现

- `GetDriveTypeW`：确认卷类型并拒绝网络盘；
- `GetDiskFreeSpaceExW`：读取调用者可用字节、总字节和卷空闲字节；
- `GetVolumeInformationW`：只保留 `FILE_READ_ONLY_VOLUME` 标志；
- 结果由 Rust 直接序列化为有界证据。

容量状态规则：

- critical：可用量低于 2 GiB，或可用比例低于 2%；
- constrained：可用量低于 10 GiB，或可用比例低于 10%；
- healthy：其他已观测状态；
- unavailable：盘符不存在、目标不允许或 API 失败。

## 自动化结果

- Node Agent：130/130 passed；
- React：109/109 passed；
- Rust/Tauri：192/192 passed；
- legacy live integration：1/1 passed；
- 真实系统卷原生查询：passed；
- 路由：只执行 `system.inventory` 与 `storage.volume.inspect`；
- parser：接受 `C盘`、`D:` 与 `drive E`，拒绝多盘符、路径、UNC 和命令拼接；
- UI：显示有界容量事实，不显示卷标或文件名。

## 真实打包桌面流程

### 真实 C: 容量压力

请求：

> C盘空间不足，帮我根据真实证据检查并清理；不要联网，不要删除任何文件。

结果：

- `C:` 可用约 3.5 GiB；
- 已用 98.2%；
- 诊断为严重容量压力；
- evidence id：`evidence-637941bdc4ae913d`；
- 当前轮只有 `system.inventory` 与 `storage.volume.inspect`；
- 无联网授权、无修复审批、无文件枚举、无删除和无系统修改。

### 不存在的 Z:

请求：

> Z盘空间不足，请根据真实证据检查；不要联网，不要删除任何文件。

结果：

- 明确显示“指定盘符不存在”；
- evidence id：`evidence-88685756a56538ac`；
- 诊断状态为安全停止；
- 文案明确“未把失败误报为空间充足”；
- 没有回退到泛系统、网络或文件扫描。

## 候选摘要

- 不可变文件：19,660；
- 不可变总字节：2,574,007,690（2.397 GiB）；
- `UCode.exe`：19,835,904 bytes；
- `UCode.exe` SHA-256：`E921BFE145B39E16ABCA52D9EDE6080B31CBD64765934BC4AA6C01BBFEC51398`；
- `Verify-UCode.ps1`：`verified=true`，零失败。

## 当前边界与下一步

本能力只证明容量状态，不证明磁盘硬件健康或文件系统完整。自动清理需要按目标类型分别建立来源证明、保留策略、预览、审批、写后验证和恢复契约；在这些条件满足前，UCode 不应以“释放空间”为由开放通用删除或任意管理员 Shell。
