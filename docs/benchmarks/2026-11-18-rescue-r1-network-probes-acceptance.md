# UCode Rescue R1 一次性联网探测正式验收报告

日期：2026-11-18

平台：Windows 11 专业版 build 26100，x64

对象：`0.3.0-capsule` 候选便携包

## 结论

本轮完成了两条真实打包桌面闭环：用户批准后，仅对明确目标执行一次 TCP 握手并保存真实证据；用户选择保持离线后，不执行 DNS/TCP 探测且不生成联网证据。审批前、批准后与拒绝后的状态均由便携数据库和证据文件独立复核。

验收只证明 `network.dns.resolve@1` 与 `network.tcp.probe@1` 在本文边界内可用。它不证明 UCode 可以访问网页、扫描网络、自动修改网络配置或解决所有网络故障。

## 实现范围与安全边界

- R1 探针注册项：`network.dns.resolve@1`、`network.tcp.probe@1`。
- 目标必须由用户消息明确给出；每次提案只绑定一个域名或一个 `host:port`。
- DNS 最多返回 8 个地址；TCP 最多尝试 4 个解析地址，每个连接最长 3 秒。
- TCP 只建立握手，不发送 HTTP 或其他应用层内容。
- 不做端口扫描、通配符目标、后台探测或长期联网授权。
- 提案绑定 ticket、工具版本、风险级别、规范化参数、nonce、过期时间与 SHA-256 摘要。
- 拒绝、过期、防重放、错误 ticket、提案篡改与非法目标均安全停止。
- 负面探测结果也作为真实 observation 保存，不把“无法连接”伪装成执行失败或成功修复。
- 记录保存在便携目录 `data/evidence/probes/<proposal-id>.json`。

## 真实桌面发现的缺陷

首次从打包后的 `UCode.exe` 输入：

> https://example.com 无法打开，请根据真实系统证据诊断；联网探测前必须让我确认。

应用错误地把 URL 中的 `/example.com` 识别为项目路径，回答不能读取项目外路径，没有进入 Rescue 流程。根因是桌面确定性路由先执行通用文件路径启发式，URL 的斜杠触发了误判。

修复后，包含明确 DNS/TCP 探测目标的 Rescue 请求优先进入系统诊断；只有不属于 Rescue 的输入才继续使用项目文件路径启发式。新增回归测试覆盖 URL、域名和显式端口请求。重新构建并替换便携候选后，以下两条真实桌面流程均通过。

## 批准路径：精确 TCP 探测

真实请求：

> 重新执行： https://example.com 无法打开，请根据真实系统证据诊断；联网探测前必须让我确认。

审批前：

- UI 展示 `network.tcp.probe`、目标 `example.com:443`、数据边界和“保持离线”选项。
- 提案 `probe-22f406ef-007b-457f-a088-b1f8d21d0dd6` 状态为 `awaiting_approval`。
- ticket 为 `ticket-af8686ab-79d3-4822-b023-5a660af2cdc8`。
- 持久化记录的 `evidence` 为 `null`，证明提案本身没有冒充已执行探测。

批准后：

- 真实建立到 `104.20.23.154:443` 的 TCP 连接。
- 证据 ID 为 `evidence-bda6dd0331cbb25a`，状态为 `observed`，耗时 2,290 ms。
- 结果记录解析地址 `104.20.23.154`、`172.66.147.243`，但只报告实际连接的单个地址。
- SQLite 恰好出现 1 条 `rescue.network_probe.proposed`、1 条 `rescue.network_probe.observed` 和 1 条基于宿主证据生成的 `provider.message`。
- 没有发送 HTTP 请求，没有修改 DNS、代理、路由或网络适配器。

## 拒绝路径：保持离线

真实请求：

> example.org DNS 解析失败，请只根据真实证据诊断；出现联网许可时我将保持离线。

结果：

- UI 展示一次性 `network.dns.resolve` 提案和精确目标 `example.org`。
- 点击“保持离线”后，提案 `probe-f7368526-9a0a-4c18-8a09-6f0c91f96c5a` 状态为 `rejected`。
- ticket 为 `ticket-a177aafd-e6e9-4a9e-b108-3e5709382979`。
- 记录中的 `evidence` 保持 `null`，detail 明确写为“没有发起 DNS 或 TCP 网络请求”。
- SQLite 恰好出现 1 条 `rescue.network_probe.proposed`、1 条 `rescue.network_probe.rejected`、0 条 `rescue.network_probe.observed` 和 0 条对应的宿主探测结果消息。

两个新 ticket 的全部持久化事件中，Unicode replacement character `�` 数量为 0。

## 自动化与构建矩阵

| 检查 | 结果 |
|---|---:|
| Node Agent / Executor | 130 passed |
| React / Vitest | 103 passed |
| Rust / Tauri | 147 passed |
| Rust `legacy_live` | 1 passed |
| TypeScript | passed |
| Vite production build | passed |
| Cargo fmt check | passed |
| `git diff --check` | passed |
| Tauri release build | passed |
| portable staging | passed |
| capsule integrity | 19,660 files, 2,573,579,146 bytes, 0 failures |

最终候选 `UCode.exe` 为 19,407,360 bytes，SHA-256：

```text
AD75364389201E50F63DA562B0D0D7349EA49314DFE7146FB34889AC702DC1E4
```

最终 staging 后又从精确路径 `E:\windows-export\UCode Capsule Candidate\UCode.exe` 执行冷启动。Windows 返回唯一的 `UCode Portable` 窗口；应用从 `tauri.localhost` 自定义协议加载而非网页开发服务器，历史 R1 拒绝记录可见，本地运行时随后进入“离线可用”，任务输入恢复可编辑。

## 当前边界与下一步

- R1 是经同意的只读联网探测，不是网络修复；不能据此声称网页内容正常或应用层服务健康。
- R0/R1/R2 已各有受限注册能力，但故障覆盖仍不完整。下一批高价值只读探针是有界 Windows Event Log 查询、文件摘要与权限检查。
- 短生命周期、最小权限的 R3 管理员 helper 尚未实现；机器级网络修复、驱动、系统服务写入和任意 PowerShell 继续禁止。
- 真实 U 盘拔插、盘符变化后运行以及第二台 Windows 机器仍需独立实机验收。
- 当前 1.5B 本地模型适合有界任务；宿主 Harness 能保证授权和证据边界，但不能替代强模型的开放式故障推理能力。
