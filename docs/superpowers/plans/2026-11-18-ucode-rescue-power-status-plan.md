# UCode Rescue 电源与电池状态诊断实施计划

日期：2026-11-18

## 1. 原生探针模块

- 新增 `desktop/src-tauri/src/rescue_power.rs`；
- 实现严格请求识别和 Windows `GetSystemPowerStatus` 读取；
- 规范化交流电、充电、临界、节电、百分比和未知寿命字段；
- 增加参数拒绝、真实 Windows 查询、标志解析、结构异常和隐私测试。

## 2. Rescue 路由与分析

- 注册 `system.power.inspect@1`；
- 明确电源请求只运行 inventory + power probe；
- 使用独立分析函数，避免泛系统假设污染结果；
- 对无电池与插电未充电保持诚实边界，不把当前快照扩大为硬件故障；
- 补充桌面确定性路由，拒绝解释性电池问题误触发。

## 3. 桌面事实投影

- 只为该工具传递有界 structured evidence；
- 显示供电来源、电量、充电、临界与无电池状态；
- unavailable 不渲染伪造的 0%；
- 增加 React 正常、无电池、失败与隐私回归。

## 4. 正式验证

- Cargo fmt/check 与 `git diff --check`；
- Node、React、Rust、真实 legacy integration 全量测试；
- 构建便携候选并运行完整性验证；
- 真实桌面分别提交中文电源请求和英文改述；
- 展开活动事实，验证只运行两个 R0 工具且无联网、审批或修改；
- 查询 SQLite 事件链，核对新 evidence ID、工具集合与零 Effect。

## 5. 文档与发布

- 更新 README 能力矩阵；
- 新增电源状态正式验收报告，记录真实值、evidence IDs、测试计数与候选摘要；
- 使用 Conventional Commit 提交实现并推送 `origin/main`；
- 验证本地与远程 HEAD 一致、工作区干净。
