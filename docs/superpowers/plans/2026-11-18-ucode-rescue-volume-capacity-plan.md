# UCode Rescue 单卷容量诊断实施计划

日期：2026-11-18

## 1. 建立独立原生模块

- 新增 `desktop/src-tauri/src/rescue_volume.rs`；
- 实现严格盘符解析、Windows API 查询、有界 JSON 与分析函数；
- 增加真实系统卷只读测试、阈值测试和对抗输入测试。

## 2. 接入 Rescue 注册表与确定性路由

- 注册 `storage.volume.inspect@1`；
- 将明确容量请求归类为 `storage`；
- 确保只执行 inventory + volume probe；
- 失败时保持 unavailable，不回退为虚假的健康结论。

## 3. 接入桌面证据投影

- 仅把该工具的有界 structured evidence 写入运行时事件；
- 添加容量事实活动文案；
- 增加隐私与主题无关的 React 回归测试。

## 4. 正式验证

- 运行 `git diff --check`、Cargo fmt；
- 运行 Node、React、Rust 和 live integration 全量测试；
- 构建便携候选并运行完整性验证；
- 在真实打包桌面中提交自然语言容量请求；
- 验证只读、无联网、无审批、无文件枚举和无删除。

## 5. 文档与发布

- 更新 README 能力矩阵；
- 新增正式验收报告；
- 提交并推送到 `origin/main`。
