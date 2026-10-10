# Contributing to odek

Thanks for wanting to help. Bug reports, small fixes and documentation
improvements are the most useful contributions, and all of them are welcome.

## Before you start

- **Bugs:** open an [issue](https://github.com/HikvIneH/odek/issues/new/choose)
  with your odek and macOS versions and how to make it happen. For terminal
  problems, the shell, prompt and program you were running help a lot.
- **Features:** open an issue first and say what you'd use it for. odek aims to
  stay small and light on memory, so not every good idea fits, and it's better
  to find that out before you write the code.
- **Security problems:** don't open a public issue; see [SECURITY.md](SECURITY.md).

Issues labelled
[good first issue](https://github.com/HikvIneH/odek/labels/good%20first%20issue)
are a good place to start.

## Building

You need an Apple Silicon Mac with macOS 12 or later and a Rust toolchain
(`cargo`).

```sh
git clone https://github.com/HikvIneH/odek.git
cd odek
cargo run                     # run from the tree
scripts/bundle.sh --install   # build and install ~/Applications/Odek.app
```

[docs/development.md](docs/development.md) covers testing, off-screen
snapshots, how odek works and where everything lives.

## Pull requests

1. Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings` (with and
   without `--features selftest`) and `cargo test`; CI runs the same on macOS.
2. For UI changes, run `scripts/termsnap.sh` or `scripts/selftest.sh`, check
   the snapshots and add a screenshot to the PR.
3. Keep the footprint in mind: a feature that adds resident memory or startup
   time needs a good reason, and the PR should say how much.
   [docs/performance.md](docs/performance.md) is the baseline: compare against
   it, and update it when a change moves the numbers.
4. Keep a PR to one change, and add a line to [CHANGELOG.md](CHANGELOG.md)
   under *Unreleased* when users would notice it.

By contributing you agree that your work is released under the
[MIT License](LICENSE), and that you'll follow the
[Code of Conduct](CODE_OF_CONDUCT.md).
