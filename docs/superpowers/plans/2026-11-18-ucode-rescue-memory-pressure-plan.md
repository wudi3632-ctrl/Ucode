# UCode Rescue 内存压力诊断实施计划

日期：2026-11-18

## 1. 原生探针模块

- 新增 `desktop/src-tauri/src/rescue_memory.rs`；
- 实现请求识别、Windows 内存状态与进程工作集查询；
- 对结果做排序、数量限制和隐私裁剪；
- 增加真实 Windows 查询、阈值和失败测试。

## 2. Rescue 路由

- 注册 `system.memory.inspect@1`；
- 明确内存请求只运行 inventory + memory probe；
- 使用独立分析函数，不让泛系统假设污染结果；
- unavailable 保留为未知。

## 3. 桌面事实投影

- 只为该工具传递有界 structured evidence；
- 显示负载、可用物理内存与最大工作集进程；
- unavailable 不渲染为零可用内存；
- 增加 React 隐私回归。

## 4. 正式验证

- Cargo fmt 与 diff check；
- Node、React、Rust、live integration 全量测试；
- 构建便携候选并运行完整性验证；
- 真实桌面提交“内存占用高”和“不要结束进程”的自然语言请求；
- 验证只运行两个 R0 工具且无联网、审批或修改。

## 5. 文档与发布

- 更新 README 能力矩阵；
- 新增正式验收报告；
- 创建 Conventional Commit 并推送 `origin/main`。
