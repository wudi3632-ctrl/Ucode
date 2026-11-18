# UCode Rescue Phase 2 — R2 可逆修复设计

## 范围

Phase 2 只引入无需管理员权限、可精确快照、可独立验证、可安全回滚的 R2 修复。首批操作是：

1. `system.user_path.normalize@1`：规范化当前 Windows 用户的 PATH，删除空项、重复项和明确不存在的绝对目录；不触碰机器 PATH。
2. `file.utf8.restore@1`：保留在 Registry 但本阶段禁用。没有已验证的“已知良好内容”时，文件恢复不能由模型猜测；后续只允许从明确备份或用户选择的来源创建方案。

WinHTTP、服务、机器级环境变量和其他需要管理员权限的修复属于 R3，不得进入 R2 执行器。任意 PowerShell/Shell、注册表泛写和模型生成命令不属于 Repair Operation。

## 权限与状态机

模型只能解释证据或建议操作类型。宿主根据已注册操作创建 `RepairProposal`，包含操作与 schema 版本、规范化参数、证据 ID、前置状态摘要、快照方案、预期后置条件、验证探针、回滚约束、nonce、创建/过期时间和 proposal digest。

状态流为：

`awaiting_approval → applying → repaired_unverified → verified → rollback_available → rolled_back`

任一前置条件、摘要、nonce、版本或过期校验失败都进入 `safe_stop`。验证失败保留 `repaired_unverified`，不能显示任务完成。回滚仅在当前状态仍等于已应用状态摘要时执行，避免覆盖审批后产生的新修改。

## 快照与存储

每次应用前把不可变 `RepairSnapshot` 写入便携数据目录 `data/evidence/repairs/<proposal-id>.json`。快照包含 before state、target fingerprint、操作/schema 版本和 SHA-256。使用临时文件 + rename 原子写入。应用与回滚结果分别记录摘要和时间。

## 桌面交互

诊断发现可安全修复的用户 PATH 问题时，显示单张结构化审批卡：原因证据、精确目标、预计改变、验证方式和回滚保障。按钮只有“暂不修复”和“批准并验证”。执行后显示真实验证结果；仅 verified 状态提供回滚入口。

## 正式验收

- 未审批不能执行；过期、重放、篡改、未知操作拒绝。
- apply 前状态变化拒绝。
- 中途失败恢复快照；验证失败不报完成。
- rollback 精确恢复 before digest；after state 变化时拒绝回滚。
- R3 操作永远不能进入 R2 执行器。
- Rust、React、Node、TypeScript、Vite、Tauri release 全部通过。
- 打包桌面真实完成诊断 → 审批 → 应用 → 验证 → 回滚，并用 UCode 外部命令独立核验系统状态。
