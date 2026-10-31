# Security policy

## Supported versions

RoomKit TUI is in early development. Security fixes land on the `main` branch;
there are no maintained release branches yet.

## Reporting a vulnerability

Please do not report security issues in public issues, discussions or pull
requests. Report them privately through GitHub's
[private vulnerability reporting](https://github.com/roomkit-live/roomkit-tui/security/advisories/new).

Include what you can of the following:

- the affected crate and commit;
- a description of the issue and its impact;
- the steps or a minimal program that reproduces it.

We will acknowledge the report, keep you informed while we work on a fix, and
credit you in the advisory unless you prefer otherwise.

## Scope

Reports of particular interest include:

- terminal control sequences from a host, model or tool reaching the terminal
  unsanitized;
- credentials, provider error bodies or another conversation's content
  reaching a view;
- a permission answer applied to a different decision, run or conversation;
- unbounded memory or CPU use driven by host, model or user input.

The libraries are not an authentication boundary: authenticating users and
authorizing their rooms, channels and actions is the host application's
responsibility.
