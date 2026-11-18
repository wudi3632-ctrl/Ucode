# UCode Rescue 电源与电池状态诊断设计

日期：2026-11-18

状态：依据已批准的 Rescue 架构与“按推荐直接实施”授权进入实现

## 目标

增加一个原生、离线、只读的 Windows 电源状态诊断，使“电池快没电”“插电但没充电”“battery not charging”“电脑突然断电”等明确请求不再退化为泛系统探针。结果必须如实说明当前是否接通交流电、电池是否存在、是否正在充电、剩余电量、系统是否标记临界电量以及是否启用节电模式。

本切片只描述当前快照，不修改电源计划，不启停设备，不读取电池序列号或厂商标识，也不把“接通交流电但未充电”直接判定为充电器、电池或主板故障。

## 方案选择

### 采用：`GetSystemPowerStatus` 原生快照

Rust/Tauri 宿主直接调用 Windows `GetSystemPowerStatus`。该 API 无需外部进程、WMI、PowerShell 或管理员权限，能够在台式机、无电池设备和笔记本之间给出稳定且有界的当前状态。

优点是依赖最小、隐私面窄、响应快，符合 U 盘便携诊断；限制是不能证明电池健康度、循环次数、瞬时掉电或充电器功率是否足够，因此这些问题必须保持 unresolved。

### 不采用：WMI `Win32_Battery`

WMI 可返回更多设备字段，但在许多机器上字段缺失或语义不一致，还会引入服务依赖、较高延迟以及不必要的设备身份信息。首个电源切片不依赖它。

### 不采用：生成 `powercfg /batteryreport`

电池报告会生成 HTML 文件、包含长期历史并扩大隐私与文件写入面。它更适合未来经用户明确批准的 R1/R2 深度诊断，而不是默认 R0 快照。

## 工具契约

- id：`system.power.inspect`
- version：`1`
- domain：`system`
- risk：`R0`
- authority：`read`
- arguments：无

结构化结果：

- `acLineStatus`：`online`、`offline` 或 `unknown`；
- `batteryPresent`：是否存在系统可见电池；
- `charging`：Windows 是否报告正在充电；
- `critical`：Windows 是否报告临界电量；
- `batterySaver`：是否启用节电模式；
- `batteryPercent`：0–100；未知或无电池时为 `null`；
- `estimatedRemainingSeconds`：Windows 有可靠估算时返回，否则为 `null`；
- `lifetimeUnknown`：明确区分未知估算与零秒；
- `rawBatteryFlagUnknownBits`：保留未知标志是否存在，但不泄露设备信息。

序列化输出上限 8 KiB。禁止返回电池序列号、设备实例路径、厂商、型号、用户名、电源计划 GUID 或历史使用记录。

## 请求识别

只在请求包含明确电源/电池故障语义时触发，例如：

- 电池电量低、电池快没电、电池不充电、插电没反应；
- 电脑突然断电、拔掉电源就关机；
- battery low、battery not charging、plugged in not charging、power status。

“什么是节电模式”“如何保护电池”“what is battery saver”等解释性问题走普通聊天。“电脑很慢”“系统异常”不足以触发电源探针。

## 分析规则

- `batteryPresent=false`：只证明 Windows 当前未发现电池；对台式机是正常事实，对声称有电池的设备保持 unresolved；
- `critical=true`，或交流电离线且电量不高于 10%：支持临界低电量；
- 交流电离线且电量不高于 20%：支持低电量；
- 交流电在线、电池存在、未充电：保持 unresolved，说明充电阈值/保护模式也可能造成该状态；
- 交流电在线且正在充电：反驳“当前完全未充电”，但不证明充电速度、适配器功率或电池健康；
- 其他已观测状态：只说明当前快照未显示低电量或供电中断；
- API 失败、百分比越界或结构不完整：unavailable，不显示伪造的 0%。

## 路由与界面

明确电源请求只执行：

1. `system.inventory`
2. `system.power.inspect`

不会同时运行 PATH、进程、服务、事件日志、网络、存储或文件探针。活动事实示例：

- `电源 已插电 · 电池 76% · 正在充电`
- `电源 使用电池 · 电池 8% · 临界`
- `电源 已插电 · 未检测到电池`
- `电源状态不可用`

UI 复用对话活动和证据收据，不新增独立 Tab；报告明确本轮没有修改电源计划或设备状态。

## 测试与正式验收

- parser 接受中英文明确电源故障表达，拒绝泛化慢和解释性问题；
- fixture 覆盖无电池、低电量、临界、插电充电、插电未充电、未知与 unavailable；
- 原生测试读取真实 Windows 状态，验证百分比、未知寿命和标志边界；
- 路由测试证明只执行 inventory + power probe；
- React 测试覆盖有电池、无电池与 unavailable，不渲染伪造 0%；
- Node、React、Rust 与真实 legacy integration 全量通过；
- 构建 portable 候选并运行完整性验证；
- 至少两轮真实桌面自然语言请求，第二轮重新读取并产生新的 evidence ID；
- SQLite 事件链确认无联网许可、审批、Effect、设备控制或系统变更。

## 非目标

- 不估算电池健康度、设计容量、满充容量或循环次数；
- 不诊断充电器瓦数、USB-C PD 协商、主板、电池鼓包或瞬时掉电根因；
- 不修改电源计划、休眠、亮度、性能模式或节电阈值；
- 不生成电池报告文件；
- 不持续后台监控。
