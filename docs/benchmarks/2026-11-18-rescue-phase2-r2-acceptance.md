# UCode Rescue Phase 2 / R2 正式验收报告

日期：2026-11-18

平台：Windows 11 专业版 build 26100，x64

对象：`0.3.0-capsule` 候选便携包

## 结论

本轮通过了“真实探针 → 只读诊断 → R2 提案 → 本地审批 → 写入后重读验证 → 精确回滚”的一条桌面闭环，并修复了 Rescue 报告在 HTTP 请求分片边界上偶发损坏中文的问题。

验收只证明当前启用的 `system.user_path.normalize@1` 和 R0 系统探针达到本报告所述范围。它不证明 UCode 已覆盖所有 Windows 故障，也不代表 R3 管理员级修复、任意注册表写入、驱动修复或无人值守系统操作已经可用。

## 实现范围

- R0 只读探针：系统、用户环境变量、进程、服务以及已有网络探针。
- R2 修复注册项：`system.user_path.normalize@1`。
- 精确写入目标：`HKCU\Environment\Path`；不修改机器 PATH。
- 清理规则：空项、大小写不敏感重复项、明确不存在的绝对目录。
- 保留规则：例如 `%JAVA_HOME%\bin` 的可展开变量路径保持字面量，注册表类型保持 `ExpandString`。
- 安全约束：提案摘要绑定、ticket 绑定、nonce、过期时间、一次性审批、写入前复核、写入后验证、失败恢复和防重放。
- 回滚约束：只有当前状态仍等于本次应用后的摘要时才恢复快照，避免覆盖用户后续修改。
- 明确禁用：`file.utf8.restore@1`、`network.winhttp.reset@1`、`system.machine_path.normalize@1` 以及所有未注册 R3 操作。

## 真实桌面修复与回滚

测试请求：

> Windows PATH 和环境变量异常，请根据真实系统证据诊断；如果存在可安全回滚的用户级修复，请生成审批方案。

桌面流程与独立复核：

1. UCode 读取真实用户 PATH，生成 R2 审批卡；审批前系统没有发生修改。
2. 提案 `repair-2c113507-223e-476e-abfd-7f2b194f8b44` 的 before digest 为 `f467f51bbe649a3860932f834f43fd3db81798f396ac3a1ee1dd4b422cda823c`，预期 after digest 为 `e0f2b8eb6a3980f10d9d026bb9a34285fe5a15fbdfe83aad055622c617aee5f5`。
3. 点击“批准、应用并验证”后，桌面显示“写入已验证”。
4. 在 UCode 外使用 Windows Registry API 并指定 `DoNotExpandEnvironmentNames` 重读：值精确等于快照 after-state，类型仍为 `ExpandString`，`%JAVA_HOME%\bin` 未展开，缺失目录和尾部空项被移除。
5. 点击“回滚本次系统修改”并二次确认；桌面显示“已精确回滚”。
6. 应用外再次重读：值精确等于快照 before-state，类型仍为 `ExpandString`，被清理项和尾部空项均恢复；快照记录状态为 `rolled_back`。

快照保存在便携数据目录的 `data/evidence/repairs/<proposal-id>.json`。本轮没有执行机器级写入或管理员提权。

## UTF-8 流式链路缺陷与修复

故障表现：同一条 hypothesis 在 `rescue.diagnosis.ready` 中正确，但经过 Rust → Node Executor 后，`provider.delta` 与 `delivery.ready` 偶发把“症状核对”持久化为“症状���对”。

根因：Node HTTP 入口对每个 Buffer 分片分别进行隐式字符串转换；当 TCP 分片落在一个 UTF-8 多字节字符中间时，两个半字符分别被替换为 `�`。

修复：

- 请求入口先按原始字节累计并执行 1 MiB 上限，再一次性 UTF-8 解码和 JSON 解析。
- 非流式模型响应显式启用 Node 的 UTF-8 StringDecoder 路径。
- 新增故意从“核”字的字节中间切开的单元测试。
- 新增真实启动 Agent HTTP 服务、分两次写入请求并逐字校验 `text_delta` 和 `complete` 的集成测试。

最终候选桌面真实请求：

> 再次检查当前用户 PATH，只依据真实系统证据进行只读诊断，不要修改系统，并确保中文完整显示。

结果：桌面显示“已完成 5 项操作”，任务上下文为“系统诊断 v8”；报告逐字显示“存在未运行的自动服务，需要结合具体症状核对”。SQLite 最新 `provider.delta`（sequence 5934）与 `delivery.ready`（sequence 5935）均不含 Unicode replacement character，收据记录“没有修改系统”和“4 项系统只读证据已核验”。旧的损坏历史事件被保留，没有静默重写用户历史。

## 自动化与构建矩阵

| 检查 | 结果 |
|---|---:|
| Node Agent / Executor | 130 passed |
| React / Vitest | 102 passed |
| Rust / Tauri | 137 passed |
| Rust `legacy_live` | 1 passed |
| TypeScript | passed |
| Vite production build | passed |
| Cargo fmt check | passed |
| `git diff --check` | passed |
| Tauri release build | passed |
| portable staging | passed |
| capsule integrity | 19,660 files, 2,573,365,130 bytes, 0 failures |

最终候选 `UCode.exe` SHA-256：

```text
A6E863294234C7560F8D230F636A79583AF6E97231114BEA6B465C0C4323455E
```

Vite 仍报告主 JavaScript chunk 约 883 KiB、gzip 约 257 KiB。这是启动性能和代码拆分债务，不是本轮正确性失败。

## 当前边界与后续阶段

- Phase 2 只启用一个用户级 R2 修复；更多故障域仍需逐项注册、验证和提供回滚契约。
- 尚未实现短生命周期、最小权限的管理员 helper，因此机器 PATH、系统服务写入和其他 R3 操作继续禁止。
- “发现未运行的自动服务”只是事实线索，不应在没有具体症状和服务身份时自动修复。
- 真实 U 盘拔插、盘符变化后的整包运行以及第二台 Windows 机器仍需独立实机验收。
- 当前内置 1.5B 模型适合有界任务；Harness 能控制权限、证据和回滚，但不能补足开放式复杂推理能力。
