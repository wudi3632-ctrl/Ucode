# UCode Rescue R3 机器 PATH 正式验收报告

日期：2026-11-18

平台：Windows 11 x64

候选版：`E:\windows-export\UCode Capsule Candidate\UCode.exe`

## 结论

本轮实现并验收了首个 R3 管理员级修复：`system.machine_path.normalize@1`。模型只负责基于 R0 证据提出方案；Rust/Tauri 宿主负责固定目标、审批、一次性认证、UAC helper、写后验证、证据和回滚。

自动测试、正式 portable 构建、胶囊完整性校验以及真实打包桌面拒绝流程均通过。真实管理员写入与第二次 UAC 回滚没有自动执行，因为 Windows 安全桌面的 UAC 必须由用户本人确认。因此本报告证明“实现与安全拒绝路径可用”，但不宣称已完成真实 HKLM 写入验收。

## 已实现边界

唯一启用的 R3 操作为：

- operation：`system.machine_path.normalize`；
- version：`1`；
- target：`HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment\Path`；
- 只清理空项、规范化后重复项和明确不存在且不含环境变量的绝对目录；
- 不接受模型生成的命令、脚本、注册表路径或任意目标；
- helper 与正常桌面启动互斥；
- 每次应用和回滚都需要独立 UAC；
- helper 使用三分钟有效期、一次性 nonce、Windows Credential Manager secret 与 HMAC-SHA256；
- 请求先 claim 再执行，摘要、目标、版本、当前状态和目标状态必须全部匹配；
- helper 写入后重读，未提权宿主再独立重读；
- 验证失败时尝试恢复原快照；
- 响应丢失或独立重读失败时不伪装成“未修改”，而是保留 `repaired_unverified` 和可回滚快照；
- 回滚前再次核对当前值仍等于应用后摘要，拒绝覆盖后续用户修改。

服务、网络栈、防火墙、ACL、驱动、任意注册表和任意管理员 Shell 仍未开放。

## 真实打包桌面验收

真实输入：

> 请修复机器 PATH，先根据真实系统证据生成方案；不要自动批准，不要绕过 UAC。

结果：

- ticket：`ticket-49779ebd-267b-45e9-8fd8-df67d35db24e`；
- evidence：`evidence-7087f03cf285b634`；
- proposal：`repair-9ea2fba7-f88e-446c-a5b9-106807ce738d`；
- 真实环境探针发现 1 个确定性机器 PATH 问题；
- 桌面 UI 显示 R3、固定 HKLM 目标、UAC 要求、双重重读验证和第二次 UAC 回滚约束；
- 用户拒绝按钮显示为“暂不修复”，批准按钮显示为“批准并打开 UAC”；
- 选择拒绝后记录状态为 `safe_stop`，detail 为“用户选择暂不修复；系统没有发生变化”；
- snapshot 为 `null`，没有伪造已应用快照；
- 拒绝前后机器 PATH SHA-256 均为 `5BB99FBDF29CE48B7A7C41B4861EBCA8306CB4A6000B98B050EEC01AC8BBA523`；
- helper 进程数为 0；
- elevated 请求/响应文件数为 0；
- 没有出现 UAC，也没有 HKLM 写入。

打包窗口标题为 `UCode Portable`，WebView 文档来源为 `http://tauri.localhost/`，不是 Vite 或本地开发服务器页面。

## Helper 启动隔离

对正式候选版只传入 helper 标志但不传请求和响应路径：

- 进程退出码为 `2`；
- UCode 进程数前后均为 `1`；
- 没有启动第二个桌面窗口；
- 没有进入 Tauri 正常启动流程。

## 真实测试发现并修复的问题

安全审查发现：helper 可能已经完成写入，但响应丢失或未提权宿主复核失败。旧实现会把此类情况归为 `safe_stop`，从而可能让一次真实修改没有可用回滚入口。

现已改为：

1. helper 失败后立即由未提权宿主重读机器 PATH；
2. 等于 before 摘要时才声明安全停止；
3. 等于 after 摘要时记录 `repaired_unverified`，不宣称双重验证完成；
4. 与 before/after 都不一致或无法读取时明确要求人工检查；
5. `verified` 与 `repaired_unverified` 都可在当前 after 摘要匹配时发起受限回滚；
6. UI 为未完全验证状态提供“按快照安全回滚”；
7. 一次性 secret 保存失败会清理尚未启动的请求文件。

新增回归测试模拟“写入成功、响应丢失”，验证状态不会被误报，并能通过第二次受限 helper 恢复原始摘要。

## 自动回归矩阵

最终代码状态执行：

- Node Agent/Harness：130 passed，0 failed；
- React/Vitest：104 passed，0 failed；
- Rust/Tauri：163 passed，0 failed；
- Rust legacy live：1 passed，0 failed；
- TypeScript typecheck：通过；
- Vite production build：通过；
- Cargo fmt：通过；
- Rust release portable build：通过。

Vitest 仍打印 jsdom 的 `HTMLCanvasElement.getContext()` 已知提示；12 个测试文件和 104 个断言全部通过。Vite 报告主 JS chunk 约 886 KB，属于后续轻量化项，不影响本次功能正确性。

## 便携完整性

最终 staging 校验：

- immutable files：19,660；
- immutable bytes：2,573,758,858；
- manifest failures：0；
- `UCode.exe`：19,587,072 bytes；
- `UCode.exe` SHA-256：`A6506904D168CE2D5A81E05CA89D0AD0AA27FAC3B1BB820964CC20983FD46C96`。

## 尚未完成的验收

以下能力不能由本轮结果推断：

- 用户在 Windows 安全桌面亲自确认 UAC 后的真实 HKLM 写入；
- 写入后的 helper 与宿主双重真实重读；
- 用户再次确认 UAC 后的真实回滚；
- UAC 超过两分钟不处理时的人工桌面体验；
- 服务、网络重置、防火墙、驱动、ACL 或任意管理员修复；
- 覆盖所有 Windows 故障。

在用户亲自完成一次真实 UAC 应用和回滚前，产品只能表述为“R3 机器 PATH helper 已实现、自动测试和拒绝路径已验收”，不能表述为“真实管理员修复已完整验收”。
