# UCode Rescue Phase 2 实施计划

1. 建立版本化 Repair Registry、proposal/snapshot/result 数据结构和 digest 规范。
2. 实现持久化 Repair Manager、一次性审批、过期/重放/篡改/状态漂移防护。
3. 实现 `system.user_path.normalize@1` 和测试用可注入 Host；文件恢复操作只保留禁用 Registry 项，不由自由文本或未知内容自动生成。
4. 将可用 proposal 接入 Rescue 诊断事件、证据收据和桌面审批卡。
5. 增加 apply、verify、rollback Tauri command 与前端 bridge。
6. 覆盖安全、不变量、失败恢复、UI 和事件投影测试。
7. 执行全量测试、release build、便携候选重建和真实桌面外部复核。
