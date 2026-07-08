# Contributing to Hunea / 参与 Hunea

感谢你愿意花时间了解或改进 Hunea。Thank you for taking the time to
understand or improve Hunea.

Hunea is still in alpha and is currently maintained in an owner-led way. The
project is not trying to collect as many features as possible; it is trying to
build a clear, comfortable, and trustworthy terminal TUI experience. Passing
tests is therefore the baseline, not the whole review standard. Changes that
affect product direction, interaction design, or user experience need maintainer
alignment first.

Hunea 目前仍处于 alpha 阶段，并且主要由维护者推进方向与体验设计。这个项目的
核心不是“能跑起来的功能列表”，而是一个清晰、顺手、可信赖的终端 TUI 体验。因
此，代码是否能通过测试只是最低要求；涉及产品方向、交互方式或用户体验的改
动，都需要先和维护者对齐。

## Terms / 术语说明

- **Issue**: a GitHub thread for reporting a bug or suggesting an improvement.
  **Issue**：GitHub 上的问题或建议记录，可以用来报告 bug，也可以提出功能建
  议。
- **PR / Pull Request**: a proposed code change submitted for maintainer
  review. **PR / Pull Request**：把你的代码改动提交给项目，请维护者审核并决
  定是否合并。
- **CI / Continuous Integration**: automated checks run by GitHub Actions,
  including build, tests, formatting, linting, and security audit. **CI / 持续
  集成**：自动检查流程，这里主要指 GitHub Actions 运行的构建、测试、格式
  化、lint 和安全扫描。
- **TUI / Terminal User Interface**: the user interface inside the terminal,
  including layout, keyboard behavior, focus movement, visual hierarchy, and
  messages. **TUI / 终端用户界面**：终端里的用户界面，包括布局、按键、光标移
  动、信息层级和错误提示。
- **Provider**: a model service connected to Hunea, usually an
  OpenAI-compatible service. **Provider**：Hunea 连接的大模型服务，通常是
  OpenAI-compatible 服务。

## Product Direction / 项目方向

Hunea is a terminal AI assistant centered on TUI experience. Please open an
issue first before working on changes like these:

Hunea 是一个以 TUI 体验为核心的终端 AI 助手客户端。以下类型的改动属于产品方
向或体验决策，请先开 issue 讨论，不建议直接提交 PR：

- Layout, navigation, key bindings, focus movement, or visual hierarchy.
- Prompt flow, tool-call flow, or agent behavior.
- Provider configuration, session persistence, or release workflow.
- New long-term configuration options, abstraction layers, plugin mechanisms, or
  compatibility layers.
- Large refactors, especially changes that cross multiple crates.

小型修复可以直接提交 PR，例如文案修正、明显的 typo、局部 bug 修复、测试补充
或不会影响体验方向的维护性改动。

Small fixes are welcome as direct PRs: wording fixes, clear typos, local bug
fixes, test additions, or maintenance work that does not change the product
experience.

## Before Opening an Issue / 提交 Issue 前

Please search existing issues first. If a similar issue already exists, adding
context there is usually more useful than opening a new one.

提交前请先搜索已有 issues，确认没有相同或非常接近的内容。重复 issue 会让维
护成本变高，也容易分散讨论。

Good issues usually include:

- A clear and reproducible bug.
- A feature suggestion tied to a specific use case.
- A real point of confusion around configuration, installation, terminal
  behavior, or provider behavior.
- An observation that can help improve the TUI experience.

适合提交的 issue：

- 明确、可复现的 bug。
- 具体场景下的功能建议。
- 对配置、安装、终端表现或 provider 行为的实际困惑。
- 能帮助改善 TUI 体验的观察。

Less useful issues include:

- “Can we add X?” without a concrete use case.
- Suggestions that duplicate existing issues.
- A single issue containing several unrelated requests.
- Security vulnerability details. Please report security issues privately as
  described in [SECURITY.md](SECURITY.md).

不太适合的 issue：

- 没有具体使用场景的“能不能加一个 X”。
- 和已有 issue 重复的建议。
- 一次包含多个互不相关需求的大合集。
- 安全漏洞细节。安全问题请按 [SECURITY.md](SECURITY.md) 说明私下报告。

## Before Opening a PR / 提交 PR 前

PRs are welcome, but acceptance is not guaranteed. Maintainers review changes
based on project direction, TUI experience, maintenance cost, and code quality.

PR 可以提交，但不保证合并。维护者会根据项目方向、TUI 体验、维护成本和代码质
量综合判断。

If the change is not small, please open an issue first and align on direction.
This helps avoid spending time on an implementation that cannot be merged because
the direction is not right for the project.

如果改动不是很小，请先开 issue 对齐方向。这样可以避免你花了很多时间实现，最
后却因为方向不合适而无法合并。

When opening a PR, please tell us:

- What problem this change solves.
- What users will notice.
- Whether it affects TUI interaction, configuration format, provider behavior,
  or release workflow.
- Which checks you ran locally.

提交 PR 时请说明：

- 这个改动解决了什么问题。
- 用户会感受到什么变化。
- 是否影响 TUI 交互、配置格式、provider 行为或发布流程。
- 你运行了哪些检查。

## Local Quality Checks / 本地质量检查

Please run these commands before submitting code when possible:

提交前请尽量运行以下命令：

```bash
cargo build
cargo nextest run
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo audit
```

If `cargo-nextest` is not installed locally, `cargo test` is acceptable for an
initial check.

如果本地没有安装 `cargo-nextest`，可以先使用：

```bash
cargo test
```

Passing CI is required for merge, but it is not enough by itself. Maintainer
review is still required.

CI 通过是合并的必要条件，但不是充分条件。所有合并仍需要维护者审核。

## Documentation Strategy / 文档策略

Full user documentation is planned for a separate documentation site. This
repository keeps only the contributor, security, release, and project
coordination documents that need to live next to the code. Once the documentation
site is available, the README and repository homepage will link to it.

完整用户文档计划由独立文档站承载。这个仓库内只保留必要的贡献、安全、发布和
项目协作说明。等文档站可用后，README 和仓库主页会链接到对应站点。

## Maintainer Review / 维护者审核

Hunea is currently maintained primarily by its maintainer. For changes that
affect experience or architecture, the maintainer may ask to adjust the
implementation, narrow the scope, split the PR, or postpone merging. This is not
a rejection of the contribution itself; it is how the project keeps its direction
coherent, its interaction experience stable, and its future maintenance cost
reasonable.

Hunea 当前主要由维护者维护。对于影响体验或架构的改动，维护者可能会要求调整
实现、缩小范围、拆分 PR，或者暂缓合并。这不是对贡献本身的否定，而是为了保持
项目方向一致、交互体验稳定，以及后续维护成本可控。
