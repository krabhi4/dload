# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| latest  | :white_check_mark: |

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

If you discover a security vulnerability, please report it by opening a [GitHub Security Advisory](https://github.com/krabhi4/dload/security/advisories/new) in this repository. This allows us to discuss and address the issue confidentially before a public disclosure.

Please include:

- A description of the vulnerability
- Steps to reproduce the issue
- Potential impact (e.g. data exposure, remote code execution)
- Any suggested fixes, if applicable

You can expect an acknowledgement within **48 hours** and a resolution timeline within **7 days** for critical issues.

## Security Best Practices for Self-Hosted Deployments

- Change the default **admin credentials** immediately after first run
- Do not expose DLoad on a public network without a reverse proxy (e.g. Nginx/Caddy) and TLS
- Keep your Docker image updated to the latest version
- Restrict the download directory permissions appropriately
