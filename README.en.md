# hunea

English | [中文](README.md)

A terminal-based AI assistant client built with Rust + Ratatui.

## Getting Started

### Configuration

Configured via three TOML files, merged user-level → project-level (`config.toml`, `models.toml`, `phrases.toml`). Copy the `*.example.toml` files from the repo root to `~/.config/hunea/` (user-level) or `./.hunea/` (project-level) and edit as needed.

## Acknowledgments

- [OpenAI Codex CLI](https://github.com/openai/codex) — Apache-2.0
- [Pi](https://github.com/earendil-works/pi) — MIT
- [Ratatui](https://github.com/ratatui/ratatui) — MIT

## Disclaimer

hunea's agent mode executes bash commands and performs file read / write / edit operations, some of which may be irreversible. Before use, please note:

- Run it in a controlled, rollback-capable environment; back up important data or keep it under version control.
- Review each tool-call approval prompt and watch for destructive commands (e.g. file deletion, `git push --force`).
- Conversation content and workspace files are sent to third-party LLM services; be mindful of data sensitivity and API costs.

This software is provided "as is", without any express or implied warranty. The author and contributors are not liable for any direct or indirect damages resulting from the use of this tool.

## License

[Apache License 2.0](LICENSE), Copyright 2026 yuewei.
