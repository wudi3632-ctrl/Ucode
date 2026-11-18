# UCode Rescue R2 当前用户手动代理正式验收报告

日期：2026-11-18

平台：Windows 11 x64

候选版：`E:\windows-export\UCode Capsule Candidate\UCode.exe`

## 结论

本轮实现并验收了 `network.user_proxy.disable_manual@1`：当真实只读探针确认当前用户手动代理已启用，且用户明确要求修复代理时，UCode 可以生成一个固定目标、需要本地审批、可验证且可精确回滚的 R2 修复方案。

自动回归、正式 portable 构建、胶囊完整性校验、真实本地模型 live Agent 路径和真实打包桌面拒绝流程均通过。为了不破坏当前正在使用的代理，本轮真实桌面测试选择“暂不修复”，没有批准写入；因此本报告证明提案、审批边界和拒绝路径可用，不把模拟宿主测试冒充为真实注册表写入验收。

## 能力与安全边界

- operation：`network.user_proxy.disable_manual`；
- version：`1`；
- risk：R2，不需要 UAC；
- 唯一目标：`HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings\ProxyEnable`；
- 仅在 `ProxyEnable == 1` 时生成修复方案；
- 应用值固定为 DWORD `0`，模型不能提供注册表路径、值或脚本；
- 快照只记录 `ProxyEnable` 的存在状态和 DWORD 原值；
- 写入后通过 Windows Registry API 独立重读并核对摘要；
- 只有当前状态仍等于本次应用后摘要时才允许回滚；
- 写入与回滚后发送固定的 WinINet settings-changed/refresh 通知；
- 不读取到提案、模型上下文或 UI，也不修改代理服务器地址、绕过列表和 PAC 地址；
- 不修改 WinHTTP 代理、环境代理变量、防火墙、Winsock 或网络适配器；
- “只检查代理，不要修复”保持只读，不生成修复审批。

`network.proxy.inspect` 也已收紧为布尔存在性证据，只返回 WinHTTP、环境代理、WinINet 手动代理、代理服务器存在性和 PAC 存在性，不返回真实端点文本。

## 真实打包桌面拒绝验收

真实输入：

> 浏览器无法联网，请根据真实证据修复当前用户手动代理；先生成方案，不要自动批准。

真实结果：

- ticket：`ticket-49779ebd-267b-45e9-8fd8-df67d35db24e`；
- evidence：`evidence-ff9847ef685e0e72`；
- proposal：`repair-60cb2028-f418-400b-be88-2ed750656835`；
- 桌面 UI 显示 R2、“禁用当前用户手动代理”和固定 `ProxyEnable` 目标；
- UI 明确说明不会删除代理服务器地址、绕过列表或 PAC 配置；
- 批准前真实 `ProxyEnable` 存在且为 `1`；
- 选择“暂不修复”后 proposal 状态为 `safe_stop`；
- detail 为“用户选择暂不修复；系统没有发生变化”；
- snapshot 为 `null`，没有伪造已应用快照；
- 拒绝前后有界代理状态 SHA-256 都为 `8c96adce7bde3ec842b09900f12303f6a760be2bbd52cccbc31ed5bfd2f81210`；
- ProxyServer 存在性和不可逆向的端点摘要前后相同，PAC 仍不存在；
- 没有 UAC，没有 elevated request/response 文件，也没有 helper 模式的 UCode 进程；
- 任务收据显示“没有修改系统”和“5 项系统只读证据已核验”。

为保护本机网络隐私，报告不记录真实 ProxyServer、PAC、WinHTTP 或环境代理文本。

## 自动化与真实服务测试

最终代码状态执行：

- Node Agent/Harness：130 passed，0 failed；
- React/Vitest：105 passed，0 failed；
- Rust/Tauri：168 passed，0 failed；
- Rust `legacy_live`：设置 `UCODE_LEGACY_LIVE=1`，连接候选包实际 `127.0.0.1:3082` 服务，1 passed，0 failed；
- live Agent 真实创建错误的 `calculator.py`，生成待审批写入，审批后改为加法并完成测试；
- TypeScript typecheck：通过；
- Cargo fmt check：通过；
- Vite production build：通过；
- Rust release portable build：通过；
- `git diff --check`：通过。

Vitest 仍打印 jsdom 的 `HTMLCanvasElement.getContext()` 已知提示，但 12 个测试文件和 105 个测试全部通过。Vite 仍报告主 JavaScript chunk 约 886 KB，这是后续启动性能和代码拆分项，不影响本轮安全正确性。

## 便携完整性

- immutable files：19,660；
- immutable bytes：2,573,826,442；
- manifest failures：0；
- `UCode.exe`：19,654,656 bytes；
- `UCode.exe` SHA-256：`2885B91A4BF2B097936F887B0C5EF0D0C481E2CF2C531B4E50F3C1C617EB50E9`。

打包窗口标题为 `UCode Portable`，WebView 文档来源为 `http://tauri.localhost/`，不是 Vite 开发服务器或普通网页。

## 尚未完成的验收

- 尚未在真实系统代理上批准应用并再次批准精确回滚；当前用户正在使用手动代理，自动测试不应破坏其联网环境；
- 真实注册表 apply/rollback 目前由 fake host 自动测试覆盖，不能等同于真实用户环境写入验收；
- 尚未覆盖认证代理、企业组策略强制代理、WPAD、WinHTTP 与 WinINet 冲突、VPN、Winsock、适配器和防火墙故障；
- 尚未完成第二台 Windows 机器和真实 U 盘盘符变化验收；
- 不能从本轮结果推断 UCode 能解决所有浏览器无法联网问题。

下一阶段应继续增加同样有界、证据绑定、可验证和可回滚的 Rescue 故障域；不应开放任意管理员 Shell、宽泛网络重置或模型直接执行注册表命令。
