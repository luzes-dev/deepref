# Security

## Report a vulnerability

Report suspected vulnerabilities privately. Use the repository Security tab's private vulnerability reporting flow when it is enabled. If it is unavailable, contact the maintainers through an already-established private channel and ask for a secure reporting path before sending exploit details. Do not open a public issue, discussion, or pull request containing a vulnerability or credential.

Include the affected source commit or artifact digest, impact, reproduction conditions, and a private contact method. Do not access data that is not yours or publish secrets as proof.

The supported version is the latest commit on `main`. Security fixes are committed directly or merged via pull requests to `main`.

## Credentials and secrets

- Keep provider tokens, API keys, database credentials, and secrets out of Git, logs, and workflow artifacts.
- Never include real credentials or sensitive data in a reproduction. If a secret may have been exposed, rotate it immediately and treat the event as an incident.
