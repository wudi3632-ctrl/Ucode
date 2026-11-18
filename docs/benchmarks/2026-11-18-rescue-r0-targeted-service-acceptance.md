# UCode Rescue R0 定向 Windows 服务诊断正式验收报告

日期：2026-11-18

平台：Windows 11 x64

候选版：`E:\windows-export\UCode Capsule Candidate\UCode.exe`

## 结论

本轮实现并验收了 `system.service.query@1`。UCode 现在可以针对用户明确提供的内部 Windows 服务名读取真实 Service Controller 和注册表元数据，判断该服务当前是否运行，并把结论绑定到单独的 evidence id。

该能力是 R0 只读诊断，不会启动、停止、重启、修改启动类型或提升权限。它解决了旧版只能罗列“所有未运行的自动服务”、无法回答具体服务状态的问题，也为后续白名单化服务修复提供了必要证据层。

## 安全与输入边界

- 只接受 2-128 个 ASCII 字母、数字、点、短横线或下划线组成的内部服务名；
- 拒绝通配符、路径、引号注入、Shell 片段、显示名称猜测和没有明确服务名的泛化请求；
- 服务名通过固定环境变量传入固定 PowerShell 探针，不拼接模型命令；
- 使用 `Get-Service` 和固定 `HKLM\SYSTEM\CurrentControlSet\Services\<name>` 只读查询；
- 最多返回 16 个依赖和 16 个被依赖服务名；
- 返回 name、displayName、state、startMode、serviceType、canStop 和有界依赖；
- 不返回 ImagePath、命令行、账户、密码、服务二进制内容或任意注册表值；
- 输出上限 64 KiB，执行上限 10 秒；
- 服务存在但停止只形成证据，不自动启动，避免把按需服务或故障状态误当成授权。

## 真实桌面验收

真实输入：

> 服务名 EventLog 异常，请只根据真实本机证据诊断，不要修改或启动服务。

最终候选版结果：

- ticket：`ticket-49779ebd-267b-45e9-8fd8-df67d35db24e`；
- user event：sequence `6079`；
- `system.inventory` evidence：`evidence-a2ee9a9b2a3acf16`；
- `system.service.query` evidence：`evidence-c0a49348bf2ec528`；
- 服务探针摘要：`指定服务 EventLog 当前状态为 Running`；
- diagnosis：`system.requested_service_stopped` 为 `contradicted`；
- 桌面结论：指定 Windows 服务当前正在运行；
- 下一步要求具体发生时间、错误码和受影响功能，不把“异常”模板化解释为服务已停止；
- delivery 只引用上述两项当前轮次证据；
- mutationPerformed：`false`；
- 任务收据显示“没有修改系统”；
- 没有 repair proposal、UAC、服务控制写入或管理员 helper。

## 真实测试发现并修复的问题

第一版真实桌面验收虽然正确读取了 `EventLog`，但暴露了两个问题：

1. `Spool*` 会被旧解析器截断成看似合法的 `Spool`；现已要求整个服务名令牌与安全边界匹配，通配符和紧随其后的非法字符会使请求安全停止。
2. 服务名 `EventLog` 被旧关键词规则误判为“读取事件日志”，同时还执行 PATH、进程和所有停止服务扫描。首次真实请求因此采集 7 项操作，包含无关证据。

修复后，明确服务名的请求只执行系统身份和该服务两项探针；只有用户明确说“事件日志”或 `Event Log` 时才追加事件日志探针。最终真实请求的 SQLite 事件只包含两个 `rescue.probe.completed`，桌面显示 3 项操作（两项探针加诊断），没有旧轮次的 PATH、进程、泛化服务或事件日志证据泄漏到当前 delivery。

服务查询最初使用 WMI 并在本机超过 10 秒；现改为 Service Controller 加固定服务注册表元数据，真实探针稳定通过，单次桌面记录耗时 4.323 秒。后续仍可通过常驻宿主或并行只读探针继续优化启动成本。

## 自动化与构建矩阵

- Node Agent/Harness：130 passed，0 failed；
- React/Vitest：105 passed，0 failed；
- Rust/Tauri：173 passed，0 failed；
- 真实 `EventLog` 服务探针：passed；
- 服务名解析、通配符/路径/注入拒绝：passed；
- 定向诊断只运行 inventory + service query：passed；
- `EventLog` 服务名不触发事件日志扫描：passed；
- TypeScript typecheck、Vite production build、Cargo fmt：passed；
- Rust release portable build：passed；
- portable staging 和胶囊完整性：passed。

Vitest 仍有 jsdom `HTMLCanvasElement.getContext()` 已知提示，全部 105 个测试通过。Vite 仍报告约 886 KB 主 JavaScript chunk，属于后续轻量化项。

## 便携完整性

- immutable files：19,660；
- immutable bytes：2,573,776,266；
- manifest failures：0；
- `UCode.exe`：19,604,480 bytes；
- `UCode.exe` SHA-256：`E53E589664DFA050041689C27C29F851F3C7AF298B0E22C33BFB03BB268FC54A`。

## 当前边界

- 本轮只证明定向服务诊断，不证明 UCode 可以安全启动或修复任意服务；
- 服务显示名称仍不能替代内部服务名；
- 尚未把服务事件按服务名、时间窗口和 Service Control Manager event id 做强关联；
- 尚未实现允许列表、依赖前置检查、启动后验证和可逆停止契约；
- 驱动型服务、受保护服务、企业策略服务和安全软件服务仍不应开放修改。

下一阶段应基于本轮 evidence contract 设计极小的服务修复允许列表；在没有明确服务身份、依赖状态、故障证据和用户审批时继续只读安全停止。
