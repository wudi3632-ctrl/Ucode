# UCode 候选竞赛与任务胶囊实施计划

## 实施原则

- 每个切片先写机械测试，再接真实运行时；
- Rust 桌面宿主拥有授权、隔离、验证、裁决和写入；
- Node 只生成结构化候选，不直接修改原项目；
- 简单任务路径不得因竞赛功能回归；
- 未通过真实基准前，竞赛保持实验能力且不默认宣传收益。

## Slice 1 — 协议、状态和确定性 Judge

1. 在 `desktop/src-tauri/src/race.rs` 定义版本化 `Race`、`Candidate`、`ValidationFact`、`Verdict`、状态转换和胶囊投影。
2. 在 `desktop/src-tauri/src/storage.rs` 增加 schema v9：`races`、`race_candidates`，保存 JSON 状态并通过 Ticket 外键级联删除。
3. 实现持久化读取、写入、列表和恢复查询，不把模型输出当作完成状态。
4. 实现纯函数 Judge：硬失败过滤、无赢家、最少文件/最小 Diff 排序、稳定平局规则。
5. 增加 Rust 单元测试：合法状态、非法转换、硬失败、双通过排序、无赢家、序列化和迁移。

## Slice 2 — Node 候选生成协议

1. 扩展 `agent/executor/protocol.js`，增加 `candidate` 模式或独立候选请求结构，要求策略 ID、允许文件、验收摘要和候选预算。
2. 在 `agent/executor/stateless.js` 增加 `minimal_change` 与 `compatibility_first` 两个固定策略。
3. 候选输出只能是结构化变更批次；禁止完成声明、应用写入或选择赢家。
4. 为完整文件、接口保留、测试同步、路径修复和单次定向修复增加 Node 测试。
5. 保持现有 chat、project_read、action 协议兼容。

## Slice 3 — 隔离候选工作区

1. 将现有 Worktree 逻辑提取为可供 Race 调用的隔离工作区服务。
2. Git 项目为每个候选创建独立受管 Worktree；非 Git 项目创建受限临时副本。
3. 加入文件数、总大小、符号链接、忽略目录和项目根边界。
4. 候选 Effect 只应用到候选工作区，原项目摘要必须保持不变。
5. 增加并发、取消、崩溃恢复、清理和所有权校验测试。

## Slice 4 — 验证与竞赛编排

1. 新增 Tauri 命令：创建竞赛、读取竞赛、取消竞赛、审查赢家。
2. 根据任务上下文和验收方法决定实验性自动触发；缺少机械验收时降级。
3. 每个候选最多一次生成和一次验证后修复。
4. 在隔离工作区运行语法、接口、Diff、测试和可用的构建检查，生成 `ValidationFact`。
5. Judge 生成 Verdict；只有通过候选可以转换成原项目的 Proposed Effect。
6. 原项目摘要变化时标记 `stale`，拒绝应用。

## Slice 5 — 任务胶囊

1. 从 Race、Candidate、Verdict、Effect 和证据生成内部胶囊投影。
2. 新增 `.ucode-task` 版本化 JSON 导出，使用保存对话框选择路径。
3. 默认导出元数据、Diff、测试摘要和哈希；脱敏环境变量、密钥和绝对用户路径。
4. 首版导入只读预览，不应用文件。
5. 增加版本、未知字段、损坏文件、路径、脱敏和往返测试。

## Slice 6 — 桌面竞赛卡

1. 在 `desktop/src/domain/types.ts` 和 `desktop/src/bridge/desktop.ts` 增加 Race API 类型和演示模式数据。
2. 在 `Conversation.tsx` 中增加紧凑 `CandidateRaceCard`，不新增主 Tab。
3. 默认显示策略、模型、状态、文件数、Diff、测试和推荐理由；详情按需展开。
4. 复用现有 Diff 审查、Effect 批准和回滚入口。
5. 完成浅色/深色、缩放、窄窗口、键盘、屏幕阅读器和减少动态效果测试。

## Slice 7 — 公开基准与启用门槛

1. 在 `benchmarks/harness-comparison/` 增加 `ucode-race` Runner。
2. 建立至少二十个一次性修改任务，覆盖单文件、多文件、测试、恢复、无赢家和越界。
3. 同模型对比 Single 与 Race，报告成功、虚假完成、无赢家、越界、耗时、内存和模型调用。
4. 只有达到设计中的 15 个百分点收益门槛才默认触发，否则保留手动实验入口。
5. 将真实失败和硬件、模型、Git 提交写入公开报告。

## Slice 8 — 真实桌面验收与发布

1. 构建新的便携胶囊并在打包后的 `UCode.exe` 中测试。
2. 模拟真实用户完成：发起任务、两个候选、无赢家、赢家审查、应用、测试、回滚、导出胶囊、取消和重启恢复。
3. 检查托盘、通知、主题、窗口缩放、断网和只读 U 盘降级。
4. 更新 README 动图、能力边界和任务胶囊示例；不声明未达到的收益。

## 当前执行切片

本轮从 Slice 1 开始，完成 Race/Candidate/Verdict 协议、SQLite v9 持久化、确定性 Judge 和 Rust 测试。Slice 1 通过后再接 Node 生成，避免在没有可信状态和裁决基础时先制作 UI。
