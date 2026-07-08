Thanks for the contribution. Please use the prompts below as a conversation
guide, not as a rigid form. Short answers are fine when the change is small.

感谢你的贡献。下面的问题只是帮助维护者理解改动，不是僵硬的表单。改动很小时，
简短回答即可。

## What Changed / 改了什么

What problem does this PR solve, and what changed in the code?

这个 PR 解决了什么问题？代码上做了哪些改动？

## User Impact / 用户影响

What will users notice after this change?

用户会感受到什么变化？

If there is no user-visible change, you can simply write:

如果没有用户可见变化，可以直接写：

```text
No user-visible change. / 无用户可见变化。
```

## Experience and Scope / 体验与影响范围

Does this touch any of the following areas?

这个改动是否涉及下面这些范围？

- TUI layout, navigation, key bindings, focus movement, or visual hierarchy.
  TUI 指终端里的用户界面。
- Prompt flow, tool-call flow, or agent behavior.
- Provider configuration or provider behavior. Provider 指 Hunea 连接的大模型
  服务。
- Session persistence, local file handling, or configuration loading.
- Release workflow, npm packages, or GitHub Releases.

If it affects TUI or product experience, please link the issue where the
direction was discussed, or briefly explain the maintainer alignment.

如果涉及 TUI 或产品体验，请链接已经讨论过方向的 issue，或简要说明是否已经和
维护者对齐。

## Checks / 检查

Please list the commands you ran. If a command was not run, a short reason is
enough.

请列出你运行过的命令。如果有命令没有运行，简单说明原因即可。

```text
cargo build
cargo nextest run
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo audit
```

## Notes for Review / 给维护者的补充

Anything the reviewer should pay attention to?

有什么希望维护者重点看的地方？
