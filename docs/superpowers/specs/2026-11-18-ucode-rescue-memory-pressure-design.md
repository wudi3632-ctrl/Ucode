# UCode Rescue 内存压力诊断设计

日期：2026-11-18

## 目标

增加一个原生、离线、只读的 Windows 内存压力诊断，使“内存不足”“内存占用 90%”“out of memory”等请求不再退化为泛系统探针。结果必须同时回答：当前机器是否真的存在内存压力，以及哪些进程的工作集最大。

本切片不结束进程、不修改页面文件、不清理缓存、不读取进程命令行或完整路径，也不把模型生成的命令交给 Shell。

## 方案选择

### 采用：原生全局状态 + 有界高内存进程

Rust/Tauri 宿主调用 Windows 内存与进程 API，返回全局内存状态和最多 10 个高工作集进程。该方案可以把“系统压力”和“进程占用”放在同一证据节点中，同时保持低延迟和严格隐私边界。

### 不采用：复用 PowerShell Get-Process

现有泛系统探针按累计 CPU 排序，不代表当前内存占用；启动 PowerShell 也增加延迟和错误面。

### 不采用：自动结束进程

进程可能持有未保存数据、数据库事务或系统依赖。没有身份重验证、用户审批与恢复契约时，结束进程不符合 UCode 的可验证和可逆原则。

## 工具契约

- id：`system.memory.inspect`
- version：`1`
- domain：`system`
- risk：`R0`
- authority：`read`
- arguments：无

结构化结果：

- `memoryLoadPercent`
- `totalPhysicalBytes`
- `availablePhysicalBytes`
- `totalPageFileBytes`
- `availablePageFileBytes`
- `topProcesses`：最多 10 项
  - `pid`
  - `processName`：basename，移除 `.exe`
  - `workingSetBytes`
  - `ownerKind`：`system`、`service_host`、`user_process`、`unknown`
- `inaccessibleProcessCount`
- `truncated`

禁止返回命令行、完整可执行路径、用户名、窗口标题、环境变量、打开文件或网络连接。

## 请求识别

仅当请求包含明确内存语义时触发，例如：

- 内存不足、内存占用高、内存爆满、内存泄漏；
- out of memory、low memory、memory pressure、high memory usage、RAM usage；
- OOM。

“电脑很慢”或“进程异常”不足以触发该探针，继续使用既有系统诊断。

## 原生执行

- `GlobalMemoryStatusEx`：读取全局物理内存、可用内存、页面文件容量与负载；
- `CreateToolhelp32Snapshot` + `Process32FirstW/Process32NextW`：枚举 PID 与 basename；
- `OpenProcess` + `GetProcessMemoryInfo`：读取当前工作集；
- 无权限进程跳过并计数，不把失败当作零占用；
- 结果按工作集降序稳定排序，最多返回 10 项。

探针不启动 PowerShell、CMD、WMI 或任何外部进程。

## 分析规则

- critical：负载至少 90%，或可用物理内存低于 512 MiB；
- constrained：负载至少 80%，或可用物理内存低于 1 GiB；
- healthy：其他已观测状态；
- unavailable：原生 API 失败。

阈值用于说明当前压力，不证明某个进程存在泄漏。即使某个进程排名第一，也只陈述工作集事实。

## 路由与 UI

明确内存请求只执行：

1. `system.inventory`
2. `system.memory.inspect`

不会同时运行 PATH、服务、事件日志、网络或文件探针。活动事实显示：

- `内存已用 84% · 可用 2.3 GiB · 最大 node 1.2 GiB`
- 或 `内存探针不可用`

报告必须说明本轮没有结束进程或修改页面文件。

## 测试与验收

- parser 接受中英文内存表达，拒绝泛化“电脑慢”；
- fixture 覆盖 healthy、constrained、critical、unavailable；
- 原生测试读取真实 Windows 内存状态并验证数值边界；
- 隐私测试证明进程名无路径分隔符，且结果没有命令行、用户名或窗口标题；
- 路由测试证明只执行 inventory + memory probe；
- React 测试覆盖正常和 unavailable 活动事实；
- Node、React、Rust 与 live integration 全量通过；
- 真实打包桌面执行至少两轮自然语言请求，确认无审批、无联网、无系统修改。

## 非目标

- 不宣称检测到内存泄漏；
- 不结束、挂起或重启进程；
- 不修改页面文件；
- 不做内核池、驱动或 ETW 深度分析；
- 不持续后台采样。
