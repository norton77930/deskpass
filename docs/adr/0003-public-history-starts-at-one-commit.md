# The public repository's history starts at a single commit

Deskpass is published as a new public repository, `norton77930/deskpass`, whose history starts at a single initial commit. The full development history, internal tickets and acceptance logs stay in a private repository. We chose this because the original history carries internal process notes and details of the maintainer's own environment, and publishing a repository cannot be undone.

**Status:** accepted (2026-11-04).

## Considered Options

- **Make the existing repository public with its full history**: rejected. Its history carries internal process notes and personal environment details, and publication cannot be undone.

## Consequences

- Contributors cannot `git blame` back past the initial commit. The reasoning behind the code is carried by the ADRs in this directory and by `CONTEXT.md` instead, so design decisions need to be recorded there.
- References to internal tickets are not resolvable publicly. Public documents describe the rule or finding itself rather than pointing at a private ticket.
