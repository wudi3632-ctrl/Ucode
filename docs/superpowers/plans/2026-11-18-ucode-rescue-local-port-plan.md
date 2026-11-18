# UCode Rescue 本机端口冲突诊断实施计划

日期：2026-11-18

依据：`docs/superpowers/specs/2026-11-18-ucode-rescue-local-port-design.md`

## 目标

交付 `network.local_port.inspect@1` 的完整 R0 纵向能力：显式请求解析、真实 Windows listener 证据、假设更新、桌面事件与收据、自动测试、生产打包和真实候选桌面验收。全程不联网、不结束进程、不修改系统。

## 任务 1：固定工具与解析边界

- 新建 `desktop/src-tauri/src/rescue_local_port.rs`；
- 实现单一端口解析器，区分 localhost/loopback 与远端主机；
- 拒绝范围、列表、注入、无上下文数字和溢出；
- 在 `rescue.rs` 注册 `network.local_port.inspect@1` 为 R0/read；
- 在 `lib.rs` 注册模块。

验证：解析器表驱动测试与工具注册表唯一性测试。

## 任务 2：有界 Windows 探针

- 固定 PowerShell 脚本使用 `Get-NetTCPConnection -State Listen`；
- 端口仅通过 `UCODE_RESCUE_LOCAL_PORT` 环境变量传入；
- 输出最多 16 个稳定排序、去重 listener；
- 只返回 localAddress、pid、processName、ownerKind；
- 沿用 10 秒超时、64 KiB 输出限制和隐藏窗口；
- 非 Windows 或探针不可用时返回 unavailable，不假装空闲。

验证：fake runner schema/上限/失败测试；Windows 上启动临时 loopback listener 进行真实 occupied/available 对照测试。

## 任务 3：路由与证据图

- 本机端口请求只执行 `system.inventory` 与 `network.local_port.inspect`；
- 非本地主机继续进入现有 R1 TCP proposal；
- 本机请求不运行 adapter/route/proxy 或外网探测；
- 添加 occupied、available、unavailable 假设和 evidence edges；
- 释放端口措辞仍保持 mutationPerformed=false。

验证：相同文本症状在 listener 存在/不存在 fixture 中得到不同结论；无关探针计数为零。

## 任务 4：桌面呈现与回归测试

- Rescue 活动和最终报告使用明确的本机端口文案；
- 证据详情保持现有渐进披露，不新增 tab；
- 任务收据明确零系统修改；
- 增加 React 测试，确保进程基名/PID 可读且不展示命令行或路径。

验证：Vitest targeted + full。

## 任务 5：正式验收与发布

- Node、React、Rust 全量测试；
- Cargo fmt/check、TypeScript/Vite、Tauri release；
- 重新 stage U 盘候选并运行完整性 verifier；
- 用真实候选对临时 listener 完成 occupied/available 两轮桌面对话；
- 确认 listener 由测试自身清理且用户进程不受影响；
- 写正式 benchmark 报告，更新 README 能力矩阵；
- 提交并推送 GitHub main。

## 停止条件

- 探针无法区分失败与空闲；
- 输出包含命令行、完整路径、用户名或远端连接；
- 本机端口请求触发 R1 网络访问；
- “释放端口”导致任何进程/服务变更；
- 没有真实候选桌面证据却声称已验收。
