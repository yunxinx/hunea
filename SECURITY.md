# Security Policy / 安全策略

## Supported Versions / 支持版本

Hunea is currently in alpha. Security fixes are provided for the latest published
alpha release and the current `main` branch.

Hunea 目前处于 alpha 阶段。安全修复会面向最新发布的 alpha 版本和当前
`main` 分支提供。

| Version              | Supported |
| -------------------- | --------- |
| Latest alpha         | Yes       |
| Older alpha releases | No        |

If you are using an older alpha release, upgrade to the latest `hunea` version
available from GitHub Releases or npm before reporting a vulnerability that may
already be fixed.

如果你正在使用较旧的 alpha 版本，请先升级到 GitHub Releases 或 npm 上可用
的最新 `hunea` 版本，再报告可能已经被修复的漏洞。

## Reporting a Vulnerability / 报告漏洞

Please do not report security vulnerabilities in public issues, discussions, or
pull requests.

请不要在公开 issue、discussion 或 pull request 中披露安全漏洞。

Preferred reporting path:

1. Open the repository's **Security** tab on GitHub.
2. Choose **Report a vulnerability** or **Private vulnerability reporting**.
3. Include the affected version, operating system, reproduction steps, impact,
   and any relevant logs or proof of concept.

推荐报告路径：

1. 打开 GitHub 仓库的 **Security** 标签页。
2. 选择 **Report a vulnerability** 或 **Private vulnerability reporting**。
3. 提供受影响版本、操作系统、复现步骤、影响范围，以及相关日志或 proof of
   concept。

If private vulnerability reporting is not available in the GitHub UI, open a
minimal public issue asking for a security contact, but do not include exploit
details or sensitive information.

如果 GitHub UI 中没有 private vulnerability reporting 入口，可以创建一个最小化
公开 issue 请求安全联系方式，但不要包含利用细节或敏感信息。

## Scope / 范围

Security reports are in scope when they affect the `hunea` binary, its release
artifacts, npm installation path, configuration loading, local file handling,
provider communication, session persistence, or GitHub/npm release supply chain.

如果问题影响 `hunea` 二进制、发布产物、npm 安装路径、配置加载、本地文件处
理、provider 通信、session 持久化，或 GitHub/npm 发布供应链，则属于安全报告
范围。

Out of scope:

- Reports for unsupported old alpha versions that do not reproduce on the latest
  alpha.
- Social engineering, spam, or physical attacks.
- Vulnerabilities in third-party services that `hunea` can be configured to use,
  unless Hunea itself handles the integration insecurely.

不在范围内：

- 不受支持的旧 alpha 版本问题，且无法在最新 alpha 版本复现。
- 社会工程、垃圾信息或物理攻击。
- `hunea` 可配置使用的第三方服务自身漏洞，除非 Hunea 的集成方式本身存在不安
  全处理。

## Response Expectations / 响应预期

Maintainers aim to acknowledge valid reports within 7 days. Fix timelines depend
on severity and release risk. When a fix is available, a patched release will be
published through the normal GitHub Releases and npm release process.

维护者会尽量在 7 天内确认有效报告。修复时间取决于漏洞严重性和发布风险。修
复可用后，会通过正常的 GitHub Releases 和 npm 发布流程发布修复版本。
