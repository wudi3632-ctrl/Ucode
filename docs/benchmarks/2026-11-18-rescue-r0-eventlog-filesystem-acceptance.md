# UCode Rescue R0 事件日志与文件证据正式验收报告

日期：2026-11-18

平台：Windows 11 x64

候选版：`E:\windows-export\UCode Capsule Candidate\UCode.exe`

## 结论

本轮通过了两条真实打包桌面只读流程：

1. 查询最近 24 小时 Windows System/Application 严重或错误事件的有界元数据；
2. 对用户明确指定的单一本地文件计算 SHA-256 并检查 ACL。

两条流程都由 Tauri/Rust 宿主执行真实探针，模型只解释宿主提供的证据。任务状态最终为 `waiting_input`，收据明确记录“没有修改系统”。没有写入审批、权限提升、网络访问或伪造完成声明。

## 已实现边界

### `system.eventlog.query@1`

- 风险级别 R0，只读自动执行；
- 只查询最近 24 小时；
- 只允许 `System` 与 `Application`；
- 只保留 Level 1/2；
- 最多 30 条；
- 只返回时间、日志名、等级、事件 ID、Provider、Record ID；
- 不读取或持久化事件 Message；
- 查询失败与“没有匹配事件”分别建模。

### `filesystem.hash@1`

- 风险级别 R0；
- 只接受用户在请求中明确写出的单一本地绝对文件路径；
- 文件上限 256 MiB；
- 以流式读取计算 SHA-256；
- 回答直接显示哈希值，但不读取或展示文件内容；
- 只有用户提供可信期望值时才判断匹配/不匹配；否则仅报告 observation。

### `filesystem.permissions.inspect@1`

- 风险级别 R0；
- 使用固定 PowerShell 脚本和已验证环境变量参数，不接受任意脚本；
- 最多返回 32 条 ACL；
- 没有显式 Deny 只用于排除该方向，不声称访问一定正常。

三个探针共同拒绝 UNC、映射网络驱动器、相对路径、通配符、变量路径、符号链接与 Windows reparse point。Windows 扩展路径在显示前被规范化为普通盘符路径。

## 真实桌面验收

### 事件日志请求

真实输入：

> 检查最近 24 小时 Windows System 和 Application 事件日志中的严重或错误事件，只读取有界元数据，不要修改系统。

结果：

- ticket：`ticket-fcf1d38d-f849-475c-aca6-256ab3c133b6`；
- `system.eventlog.query` 返回 4 条真实元数据，evidence：`evidence-42eee4f3701f6e73`；
- 本轮共核验 5 项系统只读证据；
- 回答明确写明只读诊断，没有修改系统、执行修复或提升权限；
- 收据 outcome 为 `waiting_input`；
- `rescue.repair.proposed` 数量为 0；
- 所有 12 个事件 payload 的 Unicode replacement character 数量为 0。

### 文件哈希与 ACL 请求

真实输入：

> 检查 `E:\windows-export\UCode Capsule Candidate\UCode.exe` 的 SHA256 和 ACL 权限，只依据真实证据，不要修改文件。

最终候选结果：

- ticket：`ticket-286fd568-6f4d-46f0-8917-31e98876278c`；
- 文件大小：19,403,264 bytes；
- UCode 显示 SHA-256：`2257dd44d75f427afa2a3dc3a3ab2ec436f8b554f28144d108dddc3db2defa8b`；
- 独立 `Get-FileHash`：`2257DD44D75F427AFA2A3DC3A3AB2EC436F8B554F28144D108DDDC3DB2DEFA8B`；
- 两者忽略大小写后完全一致；
- ACL 探针读取 4 条规则；
- UI 与证据摘要均显示普通 `E:\...` 路径，不再显示 `\\?\E:\...`；
- `rescue.repair.proposed` 数量为 0；
- 所有 10 个事件 payload 的 Unicode replacement character 数量为 0；
- 回答没有宣称文件完整性正常，而是要求提供可信期望摘要后再判断。

## 真实测试发现并修复的问题

1. Windows canonicalize 返回的 `\\?\` 扩展路径直接暴露在 UI。修复为仅在显示层转换成普通盘符路径，安全校验仍使用规范化路径。
2. 只读事件日志请求中的“不要修改系统”仍可能因为旁路 PATH 发现而出现 R2 修复卡。现在只有明确、非否定的修复意图才允许生成修复提案，并增加回归测试。
3. 文件哈希最初只显示“已计算”，没有把摘要交付给用户。现在证据摘要和最终回答显示完整 SHA-256，并由真实桌面与独立 Windows 命令交叉验证。

## SQLite 独立复核

候选数据库：`data\ucode.db`。使用独立 sqlite3 查询 ticket、events 与 evidence receipt：

- 事件日志 ticket 包含 `system.eventlog.query` 的 `rescue.probe.completed`；
- 文件 ticket 包含 `system.inventory`、`filesystem.hash`、`filesystem.permissions.inspect`；
- 两个 ticket 都没有 `rescue.repair.proposed`；
- 两个 ticket 都没有损坏的 UTF-8 replacement character；
- 文件哈希事件摘要包含与 `Get-FileHash` 一致的完整摘要。

## 自动回归矩阵

最终代码状态执行：

- Node Harness：130 passed，0 failed；
- React/Vitest：103 passed，0 failed；
- Rust/Tauri：155 passed，0 failed；
- Rust legacy live：1 passed，0 failed；
- TypeScript：通过；
- Vite production build：通过；
- Cargo fmt check：通过。

Vitest 在 jsdom 中仍打印已知的 `HTMLCanvasElement.getContext()` not implemented 提示，但 12 个测试文件和 103 个断言全部通过；它不是失败。

## 便携完整性

最终 staging 校验：

- immutable files：19,660；
- immutable bytes：2,573,575,050；
- manifest failures：0；
- 最终 `UCode.exe`：19,403,264 bytes；
- 最终 `UCode.exe` SHA-256：`2257DD44D75F427AFA2A3DC3A3AB2EC436F8B554F28144D108DDDC3DB2DEFA8B`。

## 未覆盖能力

本报告不证明以下能力已经可用：

- 读取事件日志正文或任意日志/任意时间范围；
- 执行任意 PowerShell；
- 读取文件内容或递归目录哈希；
- 修改 ACL；
- 访问 UNC、映射盘或其他网络文件路径；
- R3/UAC 管理员级修复；
- 覆盖所有 Windows 故障类型。

下一阶段仍应是短生命周期、最小权限、操作白名单化的 R3 helper；不能把本轮 R0 证据探针扩张成通用管理员 Shell。
