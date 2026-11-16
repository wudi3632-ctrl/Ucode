# UCode Portable Foundation Prototype

这是 UCode Portable 的第一阶段桌面原型，不是正式发布版本。

## 已实现

- Tauri 2 原生 Windows 桌面壳；
- React 对话工作区与渐进式 Diff/审批界面；
- Super Agent 兼容的 Ticket、Assignment、Turn、Effect 状态定义；
- SQLite 便携数据库和单调事件序列；
- U 盘相对路径与盘符变化解析；
- 原生项目目录选择；
- 项目与 Ticket 持久化命令；
- 兼容当前 PocketMind 本地 Agent 的对话、Effect 审批和 pytest 完成链路。

## 启动

双击 `UCode.exe`。程序无需安装或管理员权限，数据写入同目录下的 `data/ucode.db`。

当前原型通过 `127.0.0.1:3082` 使用已经启动的 PocketMind 本地 Agent。自动管理模型进程、PTY、Git、Worktree、Sub-agent 和 Skill Runtime 将按照 `ucode-manifest.json` 中的阶段状态继续实现。
