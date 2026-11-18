# UCode Rescue 单卷容量诊断设计

日期：2026-11-18

## 目标

为 UCode Rescue 增加一个离线、只读、可验证的 Windows 本地卷容量诊断，使“C 盘满了”“D: 空间不足”“disk full”等高频问题不再退化为泛系统报告。

本切片只回答一个明确本地卷当前是否存在容量压力。它不扫描文件、不统计目录、不清理缓存、不删除内容、不执行磁盘修复，也不把模型生成的命令交给 Shell。

## 方案选择

### 采用：单卷原生容量探针

用户一次明确一个盘符。Rust/Tauri 宿主调用 Windows 原生 API 获取容量和卷类型，形成 R0 证据。该方案证据强、延迟低、隐私面小，并能直接用于真实 Rescue 对话。

### 不采用：枚举所有卷

全盘枚举会暴露用户未置于范围内的设备信息，且在 U 盘、映射盘、可移动介质较多时产生噪声。

### 不采用：自动清理

删除临时文件、缓存或回收站内容涉及不可逆数据风险。当前没有针对每类清理目标的来源证明、保留策略、快照和恢复契约，因此本切片不开放写操作。

## 工具契约

- id：`storage.volume.inspect`
- version：`1`
- domain：`storage`
- risk：`R0`
- authority：`read`
- arguments：`{ "drive": "C:" }`

输入必须是一个 ASCII 盘符。拒绝 UNC、映射网络盘、卷 GUID、目录路径、多个盘符、通配符、环境变量和命令拼接文本。

结构化结果最多包含：

- `drive`
- `driveType`：`fixed`、`removable`、`optical`、`ramdisk`、`unknown`
- `totalBytes`
- `availableBytes`
- `usedBytes`
- `usedPercent`
- `readOnly`

不返回卷标、序列号、文件系统名称、用户名、文件名、目录、挂载点列表或任何文件内容。

## 请求识别

请求必须同时包含：

1. 明确盘符，如 `C 盘`、`C:` 或 `drive C`；
2. 容量意图，如“满了”“空间不足”“剩余空间”“磁盘容量”“disk full”“low disk space”。

只说“电脑卡”“磁盘坏了”不能触发该探针。远程或网络盘请求安全停止。

## 原生执行

Windows 实现使用：

- `GetDriveTypeW`：拒绝 `DRIVE_REMOTE` 和无效目标；
- `GetDiskFreeSpaceExW`：读取调用者可用字节、总字节和卷空闲字节；
- `GetVolumeInformationW`：只读取 `FILE_READ_ONLY_VOLUME` 标志，不保留卷标、序列号或文件系统名称。

探针不启动 PowerShell、CMD、WMI 或外部进程。

## 分析规则

状态按绝对值与比例中更严重的一项决定：

- critical：`availableBytes < 2 GiB` 或 `availablePercent < 2%`；
- constrained：`availableBytes < 10 GiB` 或 `availablePercent < 10%`；
- healthy：其他已观测状态；
- unavailable：API 失败、盘符不存在或目标不允许。

阈值用于诊断提示，不触发清理。`unavailable` 绝不误报为健康。

## 路由与 UI

明确单卷容量请求只执行：

1. `system.inventory`
2. `storage.volume.inspect`

不会同时运行进程、服务、网络、事件日志或文件内容探针。活动事实显示：

- `C: 可用 42.3 GB · 已用 71%`
- 或 `D: 容量探针不可用`

报告必须明确本轮没有删除文件或修改磁盘。

## 测试与验收

- 解析器接受中英文真实表达，拒绝范围、路径、网络盘与注入式文本；
- 原生测试对当前真实系统卷执行一次只读查询，并验证数值边界；
- fixture 覆盖 healthy、constrained、critical、unavailable；
- 路由测试证明只运行 inventory + volume probe；
- React 测试证明活动事实可读且不暴露卷标或文件名；
- 正式 Node、React、Rust 与 live integration 全量测试通过；
- 打包候选完整性通过；
- 真实桌面对当前系统卷执行两轮自然语言诊断，确认无审批卡、无联网、无系统变更。

## 非目标

- 不计算目录大小；
- 不做 SMART、坏道、CHKDSK 或文件系统修复；
- 不清理临时文件、浏览器缓存、更新缓存或回收站；
- 不支持网络盘；
- 不声称磁盘健康，只声明容量状态。
