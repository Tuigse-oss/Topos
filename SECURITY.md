# Security Policy

Topos reads your whole file system and can move files to the Recycle Bin,
so security problems in it are taken seriously.

## Supported versions

Security fixes go into the `main` branch and the latest release.

## Reporting a vulnerability

Please do not open a public issue for a security problem. Report it privately
with **Report a vulnerability** on the repository's
[Security tab](https://github.com/Tuigse-oss/Topos/security/advisories/new).

Include what you can of:

- the version or commit, and how Topos was installed (zip, installer,
  winget, Chocolatey, Scoop, or the Microsoft Store),
- steps to reproduce, and
- the impact, for example a delete that removes something other than what
  was confirmed or skips the Recycle Bin, a crafted folder tree or scan
  cache (`%LOCALAPPDATA%\Topos\*.toposcan`) that crashes Topos or makes it
  misbehave, a path that "Run or Open" hands to something other than the
  shell as one argument, an installer that writes where it should not or
  runs Topos elevated, or memory unsafety in a dependency.

Reports are handled by the maintainer on a best-effort basis. Progress is
shared in the private advisory, and reporters are credited in the published
advisory unless they ask not to be.
