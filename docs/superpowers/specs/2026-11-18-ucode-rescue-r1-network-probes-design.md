# UCode Rescue R1 精确网络探测设计

状态：基于已批准的 UCode Rescue 架构直接实施

## 目标

在不开放任意网络访问、任意 PowerShell 或模型执行权的前提下，让 UCode 对用户明确点名的域名与 TCP 端口进行一次性真实探测。该能力用于区分“本机配置完整但 DNS 解析失败”“DNS 正常但目标端口不可达”和“本地证据仍不足”，而不是用固定脚本宣称网络已修复。

## 范围

首批 R1 工具：

1. `network.dns.resolve@1`：只解析用户请求中明确出现的单个域名，不发起 HTTP 请求。
2. `network.tcp.probe@1`：只连接用户请求中明确出现的单个主机与端口，只完成 TCP 握手，不发送应用层数据。

没有明确主机时不创建探测提案；UCode 要求用户补充受影响域名。端口必须来自 URL、`host:port` 或请求中的明确端口。禁止端口范围、批量扫描、重定向、下载和任意 URL 请求。

## 授权与持久化

宿主创建 `R1ProbeProposal`，包含 tool/version、规范化目标、用途、将发送的数据边界、超时、ticket、nonce、创建/过期时间和 SHA-256 digest。提案写入 `data/evidence/probes/<proposal-id>.json`，采用临时文件加 rename。

状态为：

`awaiting_approval → running → observed | unavailable`

用户拒绝进入 `rejected`；过期、重放、错误 ticket、篡改或未知工具进入 `safe_stop`。每个提案只能决议一次。未批准前执行器不得进行 DNS 或 TCP 网络操作。

## 执行边界

- 执行器由 Rust 宿主实现，不调用模型或 PowerShell。
- 主机必须是合法 ASCII DNS 名、IPv4 或 IPv6；拒绝 scheme、userinfo、路径、通配符和空目标。
- DNS 最多保留 8 个唯一地址。
- TCP 只尝试解析得到的前 4 个地址，单地址超时 3 秒，总结果有界。
- 结果包含结构化地址/端口、耗时、状态与内容摘要，不包含无关机器数据。
- 本轮仅观察，不修改 DNS、代理、路由、防火墙或服务。

## 桌面交互

诊断需要 R1 时显示一张“联网探测”审批卡，明确展示工具、目标、会发出的数据和不会做的事情。操作只有“保持离线”和“允许本次探测”。完成后卡片显示真实结果，并把证据加入任务收据；不会把“允许一次”保存成全局网络权限。

## 验收

- 未批准时网络执行器调用次数为零。
- 错误 ticket、过期、重放、篡改和未知工具全部拒绝。
- 无明确主机时不生成提案。
- DNS/TCP 成功与失败都形成带 digest 的真实证据，失败不能显示为修复完成。
- 收据区分提案、用户拒绝、探针 observed、探针 unavailable 与 safe stop。
- Rust、React、TypeScript、Node、release build、便携完整性和打包桌面真实流程全部通过。

## 非目标

- 不实现 HTTP 内容抓取、测速、端口扫描、Hosts/DNS 修改或防火墙修复。
- 不把 R1 同意扩展为持久网络授权。
- 不实现 R3/UAC helper；该阶段在 R1 证据闭环稳定后继续。
