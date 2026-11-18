# UCode Rescue CPU 压力诊断实施计划

日期：2026-11-18

## 1. 原生探针模块

- 新增 `desktop/src-tauri/src/rescue_cpu.rs`；
- 实现严格请求识别、Windows 双快照和 PID 创建时间绑定；
- 计算整机负载与进程 CPU 百分比，执行排序、数量限制、名称清洗和输出上限；
- 增加真实 Windows 查询、阈值、PID 复用、失败与隐私测试。

## 2. Rescue 路由与分析

- 注册 `system.cpu.inspect@1`；
- 明确 CPU 请求只运行 inventory + CPU probe；
- 使用独立分析函数，不让泛系统假设污染结果；
- unavailable 保留为未知，单次健康快照不否认间歇问题；
- 补充桌面确定性路由，拒绝泛化慢和解释性提问误触发。

## 3. 桌面事实投影

- 只为该工具传递有界 structured evidence；
- 显示整机 CPU、最大进程和采样窗口；
- unavailable 不渲染伪造的 0% CPU；
- 增加 React 正常、失败与隐私回归。

## 4. 正式验证

- Cargo fmt/check 与 `git diff --check`；
- Node、React、Rust、live integration 全量测试；
- 关闭 Cargo 增量编译再跑一次无环境噪声的正式 Rust 全量；
- 构建便携候选并运行完整性验证；
- 真实桌面分别提交中文 CPU 高占用和英文 `high CPU usage` 请求；
- 展开活动事实，验证只运行两个 R0 工具且无联网、审批或修改；
- 查询 SQLite 事件链，核对新 evidence ID、工具集合与零 Effect。

## 5. 文档与发布

- 更新 README 能力矩阵；
- 新增 CPU 压力正式验收报告，记录真实值、evidence IDs、测试计数与候选摘要；
- 使用 Conventional Commit 提交实现并推送 `origin/main`；
- 验证本地与远程 HEAD 一致、工作区干净。
