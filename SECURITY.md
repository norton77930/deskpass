# Security Policy

Deskpass lets an AI model control a Windows desktop: it can see windows, click,
type and press keys in the user's applications. A flaw that lets it act outside
what the user approved is a security issue, not just a bug.

## Supported versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | Yes       |
| < 0.1   | No        |

Fixes land on the latest 0.1.x release; earlier releases do not receive
backports.

## Reporting a vulnerability

Use GitHub private vulnerability reporting: open the **Security** tab of
[norton77930/deskpass](https://github.com/norton77930/deskpass) and choose
**Report a vulnerability**. That channel is private to the maintainer, so
reporting there does not disclose the issue publicly.

Do not open a public issue for a suspected vulnerability, and do not include
credentials, private file paths or other people's data in a report.

## What counts as a security issue

Examples of what to report:

- Bypassing per-app approval, or bypassing the risk tiers (an app in the
  blocked tier being operated, or an app in the high tier being allowed
  "always").
- Forging or tampering with the grants file so that an app is treated as
  approved, or a tier is lowered, without the user's confirmation.
- An action reaching an app that the user did not approve.
- Background actions sending keyboard or mouse input to the user's foreground
  window.
- The stop keys (Ctrl+Alt+Esc, and Esc during foreground operation) not
  stopping the AI.
- The settings page accepting a request that does not carry its one-time token.
- Logs recording typed text or window titles.

Not security issues: a background action that an app does not support (Deskpass
reports `background_unavailable`), and the behaviours listed under known
limitations in the README and the changelog. Deskpass does not defend against
a process that already runs as the same Windows user and can call the same
Windows APIs.

## What to include

- The Deskpass version (the `VERSION` file in the install folder) and your
  Windows version.
- What you believe is unsafe, and which of the examples above it is closest to.
- The smallest steps that reproduce it, and what you expected compared with
  what happened.

## What to expect

Deskpass has a single maintainer, and this is best effort. Expect an
acknowledgement and an assessment rather than a fix on a fixed schedule. There
is no bounty.
