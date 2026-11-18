# UCode Rescue CPU 压力诊断设计

日期：2026-11-18

状态：依据已批准的 Rescue 架构与“按推荐直接实施”授权进入实现

## 目标

增加一个原生、离线、只读的 Windows CPU 压力诊断，使“CPU 占用 100%”“处理器使用率很高”“high CPU usage”等明确请求不再退化为泛系统探针。结果必须回答采样窗口内整机 CPU 是否真的繁忙，以及哪些进程贡献了最多 CPU 时间。

本切片不结束、挂起、降优先级或重启进程，不修改电源计划，不读取进程命令行或完整路径，也不把模型生成的命令交给 Shell。

## 方案选择

### 采用：Windows 原生双快照

Rust/Tauri 宿主在约 500 ms 的固定窗口前后读取系统与进程 CPU 时间，以时间差计算当前整机负载和最多 10 个高 CPU 进程。两次进程快照使用 PID 与创建时间共同绑定，避免 PID 在采样期间复用后产生错误归属。

该方案比现有累计 CPU 排序更接近用户报告的“现在很卡”，同时保持低依赖、低延迟和严格隐私边界。

### 不采用：PDH 性能计数器

PDH 可以提供更多计数器，但对象/实例映射、服务依赖和本地化边界更复杂，不适合作为首个便携 CPU 证据切片。

### 不采用：ETW 持续追踪

ETW 能提供线程、调用栈和长期趋势，但资源消耗、数据规模和隐私面明显更大。UCode 当前只需要一次有界快照，不应把诊断扩展成后台监控器。

## 工具契约

- id：`system.cpu.inspect`
- version：`1`
- domain：`system`
- risk：`R0`
- authority：`read`
- arguments：无

结构化结果：

- `cpuLoadPercent`：采样窗口内整机非空闲 CPU 百分比，范围 0–100；
- `sampleWindowMs`：实际单调时钟采样窗口；
- `topProcesses`：最多 10 项；
  - `pid`；
  - `processName`：basename，移除 `.exe` 和控制字符；
  - `cpuPercent`：相对于整机总逻辑处理能力的占比，范围 0–100；
  - `ownerKind`：只在 basename 足以证明时标为 `system` 或 `service_host`，其他为 `unknown`；
- `inaccessibleProcessCount`；
- `disappearedProcessCount`；
- `truncated`。

禁止返回命令行、完整可执行路径、用户名、窗口标题、环境变量、模块、句柄、打开文件或网络连接。

## 请求识别

只在请求包含明确 CPU 语义时触发，例如：

- CPU 占用、CPU 使用率、CPU 100%、CPU 爆满；
- 处理器占用、处理器使用率；
- high CPU usage、processor usage、CPU load。

“电脑很慢”“进程异常”或单独出现“CPU 是什么”不足以触发定向探针。泛化性能问题仍应保留为未确定原因，避免把卡顿预设成 CPU 故障。

## 原生执行

- `GetSystemTimes`：读取 idle、kernel 与 user 时间；
- `CreateToolhelp32Snapshot` + `Process32FirstW/Process32NextW`：枚举 PID 与 basename；
- `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `GetProcessTimes`：读取创建、kernel 与 user 时间；
- `std::thread::sleep`：固定约 500 ms 采样窗口，不启动外部进程；
- 第二次快照仅接受 PID 与创建时间均匹配的进程；
- 无权限、已退出和新出现的进程分别跳过或计数，不把失败当作零占用；
- 结果按 CPU 降序、PID 升序稳定排序，最多返回 10 项；
- 序列化结果上限 32 KiB，超限安全失败而不是截断 JSON。

## 分析规则

- critical：整机 CPU 至少 90%；
- constrained：整机 CPU 至少 75%；
- healthy：其他已观测状态；
- unavailable：API 失败、采样总时间为零或证据无效。

阈值只描述该采样窗口。排名第一只表示窗口内 CPU 时间最多，不证明进程异常、恶意或应该被结束。

## 路由与界面

明确 CPU 请求只执行：

1. `system.inventory`
2. `system.cpu.inspect`

不会同时运行 PATH、服务、事件日志、网络、存储或文件探针。活动事实显示：

- `CPU 已用 82.4% · 最大 python 37.6% · 采样 503 ms`
- 或 `CPU 探针不可用`

报告必须说明这是一次当前快照，并明确本轮没有结束进程或修改电源设置。UI 仍复用对话活动与证据收据，不新增独立 Tab。

## 测试与正式验收

- parser 接受中英文明确 CPU 压力表达，拒绝泛化“电脑慢”和解释性提问；
- fixture 覆盖 healthy、constrained、critical、unavailable；
- 原生测试读取真实 Windows 双快照，验证整机与进程百分比边界；
- 隐私测试证明 basename 无路径分隔符和控制字符，结果无命令行、用户名或窗口标题字段；
- PID 创建时间不一致、进程退出、无权限和零系统时间均安全处理；
- 路由测试证明只执行 inventory + CPU probe；
- React 测试覆盖正常与 unavailable 活动事实；
- Node、React、Rust 与 live integration 全量通过；
- 正式 portable 构建、完整性校验和至少两轮真实桌面自然语言请求通过；
- SQLite 事件链确认无联网许可、审批、Effect 或系统变更。

## 非目标

- 不宣称检测到应用缺陷、挖矿程序、死循环或驱动问题；
- 不结束、挂起、重启或调整任何进程；
- 不修改优先级、亲和性、电源计划或散热策略；
- 不读取温度、频率、风扇、GPU 或每核计数器；
- 不持续后台采样。
