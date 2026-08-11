# Security Policy

OpenPhoenix connects AI models to local tools, files, network services, credentials, and messaging channels. Security reports are treated as sensitive, especially when a flaw could cross a workspace boundary, expose a secret, reach a private service, execute an unapproved command, impersonate a user, or compromise the update path.

## Supported versions

OpenPhoenix is currently in its first development release. Security fixes are provided for the latest release and the current `main` branch only.

| Version | Supported |
| --- | --- |
| Current `main` | Yes |
| Latest release, currently 0.0.1 | Yes |
| Older revisions | No |

Upgrade to the newest published release before reporting a bug that may already be fixed. If the latest release itself is affected, report it privately.

## Report a vulnerability privately

Use GitHub's [private vulnerability reporting form](https://github.com/Paulus1337/OpenPhoenix/security/advisories/new). This is the only designated security-reporting route. Do not disclose a suspected vulnerability in an issue, pull request, discussion, chat, or other public channel.

Include as much of the following as you safely can:

- affected OpenPhoenix version, commit, operating system, and installation method;
- affected component and required configuration;
- minimal reproduction steps or a small proof of concept;
- actual and expected behavior;
- realistic impact and the boundary that can be crossed;
- whether exploitation needs local access, a configured channel, a malicious model response, or user approval;
- logs with tokens, keys, personal data, hostnames, and message contents redacted;
- any proposed mitigation or patch.

Do not include live credentials or data belonging to another person. Use an isolated test environment. Do not test against systems or accounts you do not own or have permission to assess.

## What to expect

The maintainer will use the private advisory to coordinate questions, validation, remediation, and disclosure. Response and release timing depend on severity, reproducibility, and maintainer availability, so no fixed response deadline is promised.

Please keep the report private until the maintainer confirms that a fix and disclosure are ready. Credit is offered when desired and appropriate. The maintainer may request a CVE through GitHub for a confirmed vulnerability.

## Scope and security boundaries

High-value report areas include:

- workspace jail bypasses, path traversal, and symlink escapes;
- command-policy or approval bypasses;
- shell or tool execution without the configured confirmation;
- server-side request forgery, DNS rebinding, or bypasses of domain and private-network policy;
- credential leakage through prompts, logs, tool output, channel replies, audit records, or error messages;
- secret-store cryptography, key handling, file permissions, or unsafe writes;
- channel authentication, pairing, authorization, and cross-session isolation;
- update checksum verification bypasses;
- release, workflow, container, plugin, MCP, browser, or sandbox trust-boundary failures;
- denial of service that defeats documented limits or requires little attacker effort.

The following usually belong in a public bug report instead:

- model quality, hallucinations, or prompt behavior that crosses no enforced boundary;
- a dangerous action the user explicitly approved and that policy allowed;
- missing hardening after the operator deliberately disabled a safeguard;
- unsupported platforms or configurations;
- dependency findings with no demonstrated impact on reachable OpenPhoenix code;
- social engineering, phishing, or physical access without a product vulnerability.

When uncertain, report privately first.

## Defense model

OpenPhoenix uses layered controls, not a claim that model output is trustworthy. Defaults include workspace confinement, shell confirmation, destructive-command blocks, public-network-only web access, secret redaction, and encrypted local secret storage. Optional controls include tool confirmations, domain allow and deny lists, audit logging, policy evaluation, and Docker or Podman sandboxing.

These controls have explicit limits:

- The built-in command deny list is a guardrail, not a complete shell sandbox.
- Shell commands run on the host when sandboxing is not enabled.
- Workspace confinement can be deliberately disabled with `security.allow_outside_workspace`.
- Private and loopback network access can be deliberately enabled with `security.allow_private_network`.
- Audit logging is optional and can itself contain operational metadata.
- Third-party model providers, chat networks, MCP servers, plugins, tools, and browsers remain separate trust domains.
- Anyone who controls the host account, configuration, environment, or secret-store passphrase can act with that authority.

A report is still valuable when it shows that documented controls fail in their default configuration or can be bypassed without the operator's informed choice.

## Safe research guidelines

To protect users and maintainers:

- stop when you have enough evidence to demonstrate impact;
- minimize access to files, messages, credentials, and services;
- never retain, alter, or publish data that is not yours;
- avoid persistence, destructive commands, denial of service, and automated scanning;
- do not open unsolicited network connections from another user's installation;
- give the maintainer a reasonable opportunity to fix a confirmed issue before disclosure.

This policy does not authorize testing of GitHub, model providers, messaging services, or any third-party infrastructure. Follow each provider's rules and applicable law.

## Security-related changes

Security patches should include a regression test when it is safe to do so. Keep exploit details in the private advisory until disclosure. Before submitting a hardening change that is not sensitive, read [CONTRIBUTING.md](CONTRIBUTING.md) and run every documented local gate.
