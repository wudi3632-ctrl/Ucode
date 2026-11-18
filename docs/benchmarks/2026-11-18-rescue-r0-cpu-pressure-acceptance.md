# UCode Rescue R0 CPU 压力诊断正式验收报告

日期：2026-11-18

平台：Windows 11 x64

能力：`system.cpu.inspect@1`

风险等级：R0 / 只读

## 验收结论

UCode 已实现真实、原生、有界的 Windows CPU 压力诊断。Rust/Tauri 宿主在约 500 ms 的窗口前后读取整机与进程 CPU 时间，用差值回答当前整机负载以及窗口内 CPU 时间贡献最高的进程。模型不生成查询命令，探针不启动 PowerShell、CMD、WMI 或外部进程。

真实打包桌面执行了两轮自然语言请求。两轮均只产生 `system.inventory` 与 `system.cpu.inspect` 两项 R0 证据。第一轮采样整机 CPU 36.3%、窗口内最高为 `python` 13.6%、实际窗口 548 ms；第二轮使用 `high CPU usage` 重新采样，得到整机 38.4%、`python` 14.6%、547 ms，并产生全新的 evidence ID，没有复用第一轮结论。

两轮均未达到 UCode 的全局 CPU 压力阈值。UCode 没有把 `python` 的窗口内最高贡献解释为应用缺陷或恶意行为，也没有联网、显示 R1 许可、生成修复审批、结束进程或修改系统。

## 安全与隐私边界

- 每次请求只做一次约 500 ms 的双快照，不持续后台监控；
- 最多返回 10 个按窗口 CPU 占比排序的进程；
- 每项只包含 PID、进程 basename、CPU 百分比和可证明的粗粒度类别；
- 两次快照必须同时匹配 PID 与创建时间，避免 PID 复用造成错误归属；
- 不返回命令行、完整可执行路径、用户名、窗口标题、模块、句柄、打开文件或网络连接；
- 无法读取或采样期间退出的进程只计数，不伪造为零占用；
- 不结束、挂起、重启进程，也不修改优先级、亲和性或电源计划；
- 最高贡献进程只是一项时间窗口事实，不等于故障、恶意、死循环或应该被结束。

## 原生实现

- `GetSystemTimes`：读取 idle、kernel 与 user 时间；
- `CreateToolhelp32Snapshot` + `Process32FirstW/Process32NextW`：枚举 PID 与 basename；
- `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `GetProcessTimes`：读取创建、kernel 与 user 时间；
- Rust 使用单调时钟记录实际采样窗口，稳定排序后截断为 10 项；
- 序列化结果超过 32 KiB 时安全失败，不输出截断的无效 JSON。

状态规则：

- critical：整机 CPU 至少 90%；
- constrained：整机 CPU 至少 75%；
- healthy：其他已观测状态；
- unavailable：原生 API 失败、采样总时间为零或证据无效。

## 自动化结果

- Node Agent：130/130 passed；
- React：113/113 passed；
- Rust/Tauri：203/203 passed；
- legacy live integration：1/1 passed；
- 真实 Windows 原生 CPU 双快照：passed；
- 路由：明确 CPU 压力请求只执行 inventory + CPU probe；
- parser：接受中英文明确 CPU 压力表达，拒绝泛化“电脑很慢”和解释性提问；
- fixture：healthy、constrained、critical、unavailable 全覆盖；
- UI：正常证据显示整机负载、最高贡献进程和采样窗口；unavailable 不显示伪造的零值；
- 隐私：自动测试确认无命令行、完整路径、用户名或窗口标题字段。

## 真实打包桌面流程

### 第一轮：中文 CPU 压力请求

请求：

> CPU占用很高，请根据当前真实证据诊断；不要联网，不要结束、挂起、重启或调整任何进程。

结果：

- `system.inventory`：`evidence-8a2d57fbc44dc037`；
- `system.cpu.inspect`：`evidence-bd8c8b33397fcbc4`；
- 整机 CPU 36.3%；
- 最高窗口贡献进程：`python`，13.6%；
- 实际采样窗口：548 ms；
- 177 个无法读取的进程只记为不可访问数量，采样期间消失 0 个；
- 诊断为当前快照未达到压力阈值；
- `mutationPerformed=false`，收据显示“没有修改系统”。

### 第二轮：英文表述重新采样

请求：

> high CPU usage，请重新采样当前状态；不要结束任何进程。

结果：

- `system.inventory`：`evidence-45378d1992465c76`；
- `system.cpu.inspect`：`evidence-abfcfe1baff16b1a`；
- 整机 CPU 38.4%，`python` 14.6%，实际采样窗口 547 ms；
- 177 个无法读取的进程只记为不可访问数量，采样期间消失 0 个；
- 新摘要、digest 与 evidence ID 证明发生了重新读取；
- 当前 ticket 的 SQLite 事件链只包含两次 inventory、两次 CPU inspect、诊断与文本交付；
- `effects` 表中该 ticket 为 0 行，事件中审批、Effect、mutation 与 R1 提及均为 0。

## 候选摘要

- 不可变文件：19,660；
- 不可变总字节：2,574,122,890（2.397 GiB）；
- `UCode.exe`：19,951,104 bytes；
- `UCode.exe` SHA-256：`172B20FE11D9DFB625220A74D3E6D40CD4EF08AA15A71DB5026B0C99E6F7CFC9`；
- `Verify-UCode.ps1`：`verified=true`，零失败。

## 当前边界与下一步

该能力只证明约 500 ms 采样窗口内的整机和已成功读取进程 CPU 时间，不证明长期趋势、单个应用缺陷、恶意软件、驱动 DPC/ISR 异常、散热降频或硬件故障。对于间歇性卡顿，应在问题复现时重新采样。若未来增加持续采样或进程操作，必须单独建立数据上限、明确目标、用户审批、身份重验证与恢复契约，不能把本 R0 探针直接升级为任意进程控制。
