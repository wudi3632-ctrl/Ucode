# UCode Rescue R0 电源与电池状态诊断正式验收报告

日期：2026-11-18

平台：Windows 11 x64

能力：`system.power.inspect@1`

风险等级：R0 / 只读

## 验收结论

UCode 已实现并通过两轮真实打包桌面验收的 Windows 电源与电池当前状态诊断。Rust/Tauri 宿主直接调用 `GetSystemPowerStatus`，不启动 PowerShell、CMD、WMI 或外部进程。本次主机在两次独立采集时均报告“已插电、未检测到系统电池”；UCode 因此没有臆测电池故障，而是明确停止在“证据尚不足”。

## 安全与隐私边界

- 每次请求只读取一次当前状态，不持续后台监控；
- 只返回交流电连接、电池存在性、充电、临界、节电、电量与可用的剩余时间估算；
- 不返回序列号、设备实例路径、厂商、型号、用户名、电源计划 GUID 或历史记录；
- 未知电量、无电池和 API 失败使用 `null` 或 unavailable，不伪造为 0%；
- 不修改电源计划、亮度、休眠、性能模式、节电阈值或设备状态；
- “已插电但未充电”保持未确定，因为充电阈值和保护模式也可能导致该状态；
- 单次快照不证明电池健康度、充电器功率、硬件故障或瞬时掉电根因。

## 原生实现

- `GetSystemPowerStatus`：读取 `SYSTEM_POWER_STATUS`；
- 严格校验交流电、百分比和节电状态的原生枚举边界；
- `0xFF` 未知电池标志、`255` 未知电量和 `DWORD_MAX` 未知寿命被保留为未知；
- 结果序列化上限 8 KiB，超限安全失败。

## 自动化结果

- Node Agent：130/130 passed；
- React：115/115 passed；
- Rust/Tauri：210/210 passed；
- real legacy integration：1/1 passed（显式临时工作区，真实 approved change + pytest）；
- 真实 Windows 原生电源查询：passed；
- 路由：明确电源请求只执行 inventory + power probe；
- parser：接受中英文明确电源故障，拒绝泛化慢和解释性问题；
- fixture：无电池、低电量、临界、插电充电、插电未充电、未知与 unavailable 已覆盖；
- UI：正常、无电池与 unavailable 已覆盖，未知状态不显示伪造 0%；
- 隐私：自动测试确认无序列号、设备路径、厂商、型号、用户名或电源计划 GUID。

## 真实打包桌面流程

### 第一轮：中文电源状态请求

请求：

> 检查当前电源状态和电池是否在充电；只读取真实证据，不要修改电源计划或设备状态。

结果：passed。桌面回答显示“电源 已插电，未检测到电池”，并明确本轮没有修改电源计划。最终候选事件 `6218` 是 `system.inventory`，事件 `6219` 是 `system.power.inspect`；电源证据为 `evidence-61c08b789893ca6f`，结构化值为 `acLineStatus=online`、`batteryPresent=false`。本轮没有 Effect、R1、审批或系统变更。

### 第二轮：英文故障表述重新读取

请求：

> battery not charging，请重新读取当前真实状态；不要修改任何设置。

结果：passed。最终候选事件 `6226` 和 `6227` 分别重新采集 inventory 与 power；第二次电源探针采集时间为 `2026-11-18T15:46:24.886Z`，不是复用第一轮事件。状态内容未变化，因此内容寻址的电源 evidence ID 合理地仍为 `evidence-61c08b789893ca6f`。整个最终 ticket 共 4 个 probe 事件、2 个 delivery，Effect、R1、审批和 mutation 均为 0。

## 候选摘要

- 不可变文件：19,660；
- 不可变总字节：2,574,196,618；
- `UCode.exe`：20,024,832 bytes；
- `UCode.exe` SHA-256：`4AACADB43E3C9F68BC0D09860BE78A7B311ED1E6C5B83D4D0C6D2BC381FBA558`；
- `Verify-UCode.ps1`：verified=true，零失败。

## 当前边界与下一步

该能力只证明采样时刻的 Windows 电源状态，不证明电池容量衰减、循环寿命、充电器协议、主板供电、驱动问题或瞬时掉电。如果用户报告间歇故障，应在复现时重新读取；未来若加入长期电池报告，必须单独建立数据预览、文件写入审批、隐私上限和删除策略，不能把本 R0 探针直接扩展成后台监控。
