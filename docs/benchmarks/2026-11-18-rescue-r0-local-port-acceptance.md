# UCode Rescue R0 本机 TCP 端口诊断正式验收报告

日期：2026-11-18
平台：Windows 11 x64
能力：`network.local_port.inspect@1`
风险等级：R0 / 只读

## 验收结论

UCode 已实现针对一个用户明确指定的本机 TCP 端口的真实 listener 诊断。探针由 Rust/Tauri 宿主调用 Windows IP Helper 原生 API，不依赖模型生成命令或 PowerShell。它可区分“正在监听”“当前未发现 listener”和“探针不可用”，并将结论绑定到有界结构化证据。

本报告只证明单端口、只读诊断在下述边界内可用。它不证明 UCode 能扫描端口、终止进程、停止服务、修改防火墙或自动解决所有端口冲突。

## 固定安全边界

- 只接受 `1..65535` 的一个明确 TCP 端口；
- 只识别 `localhost`、`127.0.0.1`、`::1` 或明确“本机端口/port”冲突语义；
- 远程主机继续使用原有 R1 一次性联网授权流程；
- 每次最多返回 16 个 listener；
- 只返回本地绑定地址、PID、进程 basename 和所有者类别；
- 不返回命令行、完整可执行路径、用户名、远程连接或网络负载；
- 不扫描其他端口，不发起网络请求，不结束进程，不停止服务，不修改防火墙；
- 请求中即使写了“释放”，本轮也只诊断并明确说明没有执行系统修改。

## 实现证据

- Windows API：`GetExtendedTcpTable`，同时读取 IPv4 与 IPv6 listener 表；
- 进程名称：`OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `QueryFullProcessImageNameW`，只保留 basename；
- 路由：本机端口请求只执行 `system.inventory` 与 `network.local_port.inspect`；
- 命令层回归：本机端口请求不会创建 `network.tcp.probe` 的 R1 授权卡；
- UI：事实行显示“本机端口 … 正在监听 · 进程 · PID …”或“未发现监听者”。

## 自动化结果

- Node Agent：130/130 passed；
- React：107/107 passed；
- Rust/Tauri：188/188 passed；
- legacy live integration：1/1 passed；
- 原生真实 listener 测试：临时绑定随机 loopback 端口后识别为 occupied，关闭后识别为 available；
- 解析对抗用例：拒绝远程主机、端口 0、65536、范围与命令注入式文本；
- 隐私断言：结果中没有 `CommandLine`、路径分隔符或完整程序路径。

## 真实打包桌面流程

以测试自身拥有的 Node 进程在 `127.0.0.1:54321` 创建临时 listener，然后在真实打包的 UCode 桌面窗口中执行两轮用户流程。

1. 输入“本机端口 54321：被谁占用？请根据真实证据诊断；不要联网，不要结束任何进程。”；
2. 桌面真实显示 `node`、`PID 61540` 与 occupied 结论，证据 id 为 `evidence-47be5f419a33c3d0`；
3. 当前轮只包含 `system.inventory` 与 `network.local_port.inspect`，没有 R1 联网授权、没有修复审批、没有系统修改；
4. 只关闭测试自身拥有的 listener，未接触用户进程；
5. 输入“再次检查本机端口 54321：请根据当前真实证据说明是否仍被占用；不要联网，不要结束任何进程。”；
6. 桌面结论变为“未发现监听者”，证据 id 为 `evidence-bb5466fdd249d07d`，仍然只有两个 R0 本机工具。

第一次桌面验收还发现一个真实缺陷：端口数字后紧跟中文冒号时，解析器没有识别端口，请求退化为泛网络诊断并错误采集了适配器、路由和代理。修复后将中英文冒号纳入安全边界，但仍拒绝分号和 `Stop-Process` 式注入文本；新增回归测试并重新打包后，上述两轮桌面流程全部通过。

候选摘要：

- 不可变文件：19,660；
- 不可变总字节：2,573,982,602（2.397 GiB）；
- `UCode.exe`：19,810,816 bytes；
- `UCode.exe` SHA-256：`9E7BAACDE6918E7E89877101D5FC147A175187CBF9CCBF90C4EC6FF74C10CFAF`；
- `Verify-UCode.ps1`：`verified=true`，19,660 个文件、零失败。

## 当前能力边界

UCode 当前不会自动“释放端口”。这是有意的产品边界：同一个 PID 可能是用户正在工作的开发服务器、数据库或系统组件，未经确定身份、依赖与回滚方案就终止进程不符合 Rescue 的可验证、可逆原则。后续若增加进程终止能力，必须采用独立白名单操作、明确审批、进程身份重验证和可证明的恢复策略，不能复用本 R0 探针直接执行。
