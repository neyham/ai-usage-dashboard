# Security Policy

## Supported versions

Security fixes target the latest published release and the current `main`
branch. Older builds may not receive fixes.

## Reporting a vulnerability

Do not open a public issue for a vulnerability or suspected credential leak.
Use GitHub's private vulnerability reporting flow on the repository's
**Security** tab when it is available. Otherwise, contact the repository owner
through GitHub first and wait for a private channel before sharing technical
details. If no private contact method is available, open an issue that asks the
maintainer to enable private reporting without including vulnerability details.

Include the affected version or commit, Windows version, impact, and the
smallest safe reproduction you can provide. Sanitized logs and a proposed fix
are useful, but there is no guaranteed response-time SLA.

Appropriate reports include:

- credentials, provider responses, or sensitive account data crossing the
  renderer IPC boundary or being written to logs;
- unsafe credential-file updates, locking failures, or permission issues;
- command, argument, or path injection through configuration or WSL handling;
- a CSP bypass or another issue that exposes local data; and
- a dependency vulnerability that is reachable in this application.

Provider outages, quota discrepancies, and undocumented endpoint changes are
usually regular bug reports unless they create a security impact.

## Protect credentials when reporting

Never attach or paste any of the following:

- Claude credential files, access tokens, refresh tokens, or raw OAuth
  responses;
- Codex `auth.json`, authorization headers, cookies, or account identifiers;
- DeepSeek API keys;
- a dashboard `config.json` containing `deepSeekApiKey`;
- signing keys or certificate passwords; or
- unredacted screenshots, paths, or cache files that reveal usernames, plan
  details, balances, or other account information.

Use the built-in mock modes and clearly fake tokens for reproductions. Redact
secrets rather than partially masking them. If a real credential was exposed,
revoke or rotate it with the provider immediately and do not wait for a project
response.

The application cache contains sanitized usage and balance data rather than
provider credentials, but it can still contain private account information.
Treat it as sensitive when preparing a report.
