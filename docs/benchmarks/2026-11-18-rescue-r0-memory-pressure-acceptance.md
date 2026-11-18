# UCode Rescue R0 内存压力诊断正式验收报告

日期：2026-11-18

平台：Windows 11 x64

能力：`system.memory.inspect@1`

风险等级：R0 / 只读

## 验收结论

UCode 已实现真实、原生、有界的 Windows 内存压力诊断。Rust/Tauri 宿主直接读取当前物理内存、页面文件状态，并列出最多 10 个高工作集进程。模型不生成查询命令，探针不启动 PowerShell、CMD、WMI 或外部进程。

真实打包桌面执行了两轮自然语言请求。两轮均只产生 `system.inventory` 与 `system.memory.inspect` 两项 R0 证据；当前机器内存已用 68%、可用约 10.1 GiB，未达到 UCode 的压力阈值，最大已读取工作集为 `llama-server` 约 1.7 GiB。第二轮使用 `out of memory` 的不同表述重新采样并产生新的 evidence ID，没有复用第一轮结论。

UCode 没有把“最大工作集”解释成内存泄漏，也没有联网、显示 R1 许可、生成修复审批、结束进程或修改系统。

## 安全与隐私边界

- 一次只读取一个当前系统快照，不持续后台采样；
- 最多返回 10 个按工作集排序的进程；
- 每项只包含 PID、进程 basename、工作集字节和可证明的粗粒度类别；
- 不返回命令行、完整可执行路径、用户名、窗口标题、环境变量、打开文件或网络连接；
- 无法读取的进程只计数，不伪造为零占用；
- 不结束、挂起或重启进程；
- 不修改页面文件、缓存、服务、注册表或系统设置；
- 不声称单次工作集快照能够证明内存泄漏。

## 原生实现

- `GlobalMemoryStatusEx`：读取内存负载、物理内存和页面文件容量；
- `CreateToolhelp32Snapshot` + `Process32FirstW/Process32NextW`：枚举 PID 与 basename；
- `OpenProcess` + `GetProcessMemoryInfo`：读取当前工作集；
- Rust 对进程名移除控制字符和 `.exe`，稳定排序后截断为 10 项；
- 序列化结果超过 32 KiB 时安全失败，不输出截断的无效 JSON。

状态规则：

- critical：负载至少 90%，或可用物理内存低于 512 MiB；
- constrained：负载至少 80%，或可用物理内存低于 1 GiB；
- healthy：其他已观测状态；
- unavailable：原生 API 失败或证据无效。

## 自动化结果

- Node Agent：130/130 passed；
- React：111/111 passed；
- Rust/Tauri：196/196 passed；
- legacy live integration：1/1 passed；
- 真实 Windows 原生内存与进程查询：passed；
- 路由：明确内存请求只执行 inventory + memory probe；
- parser：接受中英文内存压力与独立 `OOM` token，拒绝泛化“电脑很慢”和英文 `room` 误匹配；
- fixture：healthy、constrained、critical、unavailable 全覆盖；
- UI：正常证据显示全局值与最大工作集；unavailable 不显示伪造的零值；
- 隐私：自动测试确认无命令行、完整路径、用户名或窗口标题字段。

## 真实打包桌面流程

### 第一轮：中文内存压力请求

请求：

> 内存占用很高，请根据真实证据诊断；不要联网，不要结束、挂起或重启任何进程。

结果：

- `system.inventory`：`evidence-5f69f63584a4fa27`；
- `system.memory.inspect`：`evidence-d0f4ca513d322286`；
- 内存负载 68%；
- 可用物理内存 10,857,902,080 bytes，UI 显示 10.1 GiB；
- 最大已读取工作集：`llama-server`，1,796,202,496 bytes，UI 显示 1.7 GiB；
- 181 个无法读取的进程只记为不可访问数量，没有当作零占用；
- 诊断为当前快照未达到压力阈值；
- `mutationPerformed=false`，收据显示“没有修改系统”。

### 第二轮：英文错误表述重新采样

请求：

> 电脑提示 out of memory，请重新读取当前真实状态；不要结束任何进程。

结果：

- `system.inventory`：`evidence-d1eaf23b0d07e261`；
- `system.memory.inspect`：`evidence-e20f0549770d4973`；
- 内存负载仍为 68%，可用物理内存 10,834,817,024 bytes；
- 生成新的摘要、digest 与 evidence ID，证明发生了重新读取；
- 当前 ticket 的 SQLite 事件链只包含两次 inventory、两次 memory inspect、诊断和文本交付；
- 没有 `effect.proposed`、R1 联网许可、审批或系统变更事件。

## 候选摘要

- 不可变文件：19,660；
- 不可变总字节：2,574,059,914（2.397 GiB）；
- `UCode.exe`：19,888,128 bytes；
- `UCode.exe` SHA-256：`8242157273C770A0F6BF12C0B077E26EDBD48FDDEED9390239E2087CF7344573`；
- `Verify-UCode.ps1`：`verified=true`，零失败。

## 当前边界与下一步

该能力证明的是采样时刻的全局内存状态和已成功读取的进程工作集，不证明长期趋势、驱动泄漏、内核池异常或单个应用存在内存泄漏。对于间歇性 OOM，应在问题复现时重新采样；若未来增加持续采样或进程操作，必须分别建立明确的时间窗口、数据上限、用户审批、身份重验证和恢复契约，不能把本 R0 探针直接升级成任意进程控制。
