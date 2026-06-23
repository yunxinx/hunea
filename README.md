# hunea

**中文** | [English](README.en.md)

基于 Rust + Ratatui 构建的终端 AI 助手客户端。

## 快速开始

### 配置

通过三个 TOML 文件配置，按用户级 → 项目级覆盖合并（`config.toml`、`models.toml`、`phrases.toml`）。将仓库根目录的 `*.example.toml` 复制到 `~/.config/hunea/`（用户级）或 `./.hunea/`（项目级）并按需修改。

## 致谢

- [OpenAI Codex CLI](https://github.com/openai/codex) — Apache-2.0
- [Pi](https://github.com/earendil-works/pi) — MIT
- [Ratatui](https://github.com/ratatui/ratatui) — MIT

## 免责声明

hunea 的 agent 模式会执行 bash 命令并对文件进行读取、写入与编辑操作，部分操作可能不可逆。请在使用前注意：

- 在受控、可回滚的环境中运行，重要数据提前备份或纳入版本控制。
- 审查每一次工具调用审批提示，留意破坏性命令（如文件删除、`git push --force` 等）。
- 对话内容与工作区文件会发送至第三方 LLM 服务，留意数据敏感性与 API 费用。

本软件按"原样"提供，不附带任何明示或默示的保证。作者与贡献者不对因使用本工具造成的任何直接或间接损失负责。

## 许可证

[Apache License 2.0](LICENSE)，Copyright 2026 yuewei。
