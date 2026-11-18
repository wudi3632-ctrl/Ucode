# UCode Rescue R3 Windows Time 服务修复正式验收报告

日期：2026-11-18

平台：Windows 11 x64

候选版：`E:\windows-export\UCode Capsule Candidate\UCode.exe`

## 结论

本轮实现并正式验证了首个白名单化 Windows 服务修复 `system.service.start@1`。它不是任意服务控制器：只允许在真实 `system.service.query` 证据存在、`W32Time` 精确为 `Stopped`、固定依赖 `RpcSs` 为 `Running` 时生成 R3 提案。

模型只负责提出意图。Rust/Tauri 持有操作注册表、固定目标、审批状态、快照、认证请求、原生 SCM 执行、独立验证和回滚。helper 不接受模型生成的命令、服务路径、显示名称、通配符或任意服务名。

自动化已证明完整的 `Stopped -> Running -> Stopped` 状态机、双重重读、认证与回滚安全边界。真实候选桌面上，本机 `W32Time` 已经是 `Running`，因此产品正确地没有生成修复卡或 UAC；没有为了测试而停止真实服务。真实管理员应用和回滚必须由用户本人分别确认两次 Windows 安全桌面 UAC，在完成前不声称真实提权变更已验收。

## 固定操作边界

- operation：`system.service.start@1`；
- risk：R3；
- 唯一服务：`W32Time`；
- 固定依赖：`RpcSs`；
- 固定目标：`SCM\W32Time`；
- apply：仅允许 `Stopped -> Running`；
- rollback：仅允许 `Running -> Stopped`；
- apply 与 rollback 各自需要独立 UAC；
- 不修改启动类型、服务账户、二进制路径、依赖、恢复策略或其他服务；
- 不调用 PowerShell/CMD 执行服务写入，helper 使用 `OpenSCManagerW`、`OpenServiceW`、`QueryServiceStatusEx`、`StartServiceW` 和 `ControlService`；
- 15 秒内未达到目标状态即失败，不伪造完成；
- helper 与未提权宿主都必须重读到同一 SHA-256 状态摘要，才标记 `Verified`；
- 当前状态不再等于快照 after 摘要时拒绝回滚，避免覆盖用户或系统的后续变化。

## 认证 helper 安全性

- 每次申请生成独立 request id、nonce 和一次性 secret；
- secret 保存在 Windows Credential Manager，响应后删除；
- 请求与响应均使用 HMAC-SHA256；
- HMAC 绑定 ticket、proposal、operation、target、before/after 状态与摘要、action、nonce 和有效期；
- 请求文件名与认证 request id 必须一致；
- helper 通过原子 rename claim 请求，重放会失败；
- 未知服务、错误目标、错误依赖、过期请求、被篡改状态或 HMAC 均在写入前拒绝；
- 写入后验证失败会尝试恢复 before 快照，并明确报告恢复成功或失败；
- UAC 取消单独记录为 `UacCancelled`，只有未提权宿主证明服务仍为 before 摘要时才显示“未修改”。

## 真实桌面流程

### 本机 W32Time 请求

输入：

> Windows Time 服务 W32Time 无法启动，请根据真实服务证据生成修复方案；不要自动批准。

候选版真实结果：

- 只运行系统身份和定向 `system.service.query`；
- evidence：`evidence-7eecfdeed47b5477`；
- 真实状态：`W32Time = Running`；
- diagnosis 将“服务已停止”假设标记为 contradicted；
- 不生成 `system.service.start` 修复卡；
- 不出现 UAC；
- 任务收据显示“没有修改系统”；
- 不启动、不停止、不重启服务，不修改启动类型。

### 非允许列表服务请求

输入：

> 服务 Spooler 异常，请修复并启动；不要自动批准。

候选版真实结果：

- 定向读取 `Spooler` 真实状态，evidence：`evidence-f7e844889c2e29a4`；
- 本机真实状态为 `Running`；
- 不生成 W32Time 或其他修复卡；
- 不把 `Spooler` 请求回退成 PATH 修复；
- 不出现 UAC，也没有系统修改。

这两条真实流程证明路由不会因出现“修复/启动”字样就执行服务写入；是否存在明确服务名、是否在允许列表、当前真实状态和依赖状态都由本地策略重新判断。

## 正式测试矩阵

- Node Agent/Harness：130 passed，0 failed；
- React/Vitest：106 passed，0 failed；
- Rust/Tauri：183 passed，0 failed；
- Rust live legacy integration：1 passed；
- R3 service helper 专项：15 passed；
- R3 repair state machine 专项：16 passed；
- 真实 Windows 原生 `W32Time` 只读 SCM 查询：passed；
- TypeScript typecheck、Vite production build、Cargo fmt/check：passed；
- Rust release portable build：passed；
- portable staging 和 SHA-256 完整性：passed；
- 真实打包桌面 W32Time/Spooler 多轮对话：passed，零系统修改。

Vitest 仍打印 jsdom `HTMLCanvasElement.getContext()` 已知提示，但 106 个测试全部通过。Vite 仍提示约 887 KB 主 JavaScript chunk，属于后续轻量化工作，不影响本轮服务安全契约。

## 便携完整性

- immutable files：19,660；
- immutable bytes：2,573,963,658；
- manifest failures：0；
- `UCode.exe`：19,791,872 bytes；
- `UCode.exe` SHA-256：`22253CC0D118A853D91D11B0619B11E769E60D2BAFBB385A57A2CB14B71194A4`。

## 当前边界

- 尚未由用户本人完成真实 `W32Time` UAC 应用和第二次 UAC 回滚，因此不能声称真实管理员服务变更已完整验收；
- 当前只支持 `W32Time`，不支持任意服务启动、停止、重启或启动类型修改；
- 服务故障与 Service Control Manager 事件 ID 的定向关联仍需继续完善；
- 驱动、受保护服务、安全软件、企业策略服务和未知依赖图继续保持只读；
- 不应为扩大覆盖而开放任意管理员 Shell、PowerShell/CMD 或模型生成的服务目标。

下一步应继续用相同原则扩展少量高价值故障域：明确证据、最小允许列表、固定状态转换、独立验证、可逆快照和真实用户 UAC，而不是追求“什么都能执行”的宽泛权限。
