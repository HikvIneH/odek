# Security policy

## Supported versions

Only the latest release of odek gets security fixes. Update with
`brew upgrade --cask odek`, or download it from
[Releases](https://github.com/HikvIneH/odek/releases).

## Reporting a vulnerability

Please don't open a public issue. Report it privately through GitHub instead:
[Security › Report a vulnerability](https://github.com/HikvIneH/odek/security/advisories/new).

Say what the problem is, how to reproduce it and which version you tested. You
can expect a reply within a week. Once a fix is released, the advisory is
published and you're credited unless you'd rather not be.

## What's in scope

odek runs your shell and the programs in it, so the interesting places are
where it handles input it didn't write:

- escape sequences and other output from programs running in the terminal
- file paths and URLs that odek turns into links, and files opened in the code viewer
- the `odek` command handing paths to the running app
- the update check, release builds and the Homebrew cask

What a program running in your shell can do with your own permissions is not a
vulnerability in odek.
