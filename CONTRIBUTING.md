# Contributing to Lumen

Thanks for helping. Bug reports, translations and pull requests are all welcome.

## Reporting bugs

Open an [issue](https://github.com/ImanolGo/lumen-rs/issues) with your distribution and desktop, the Lumen version (`lumen-rs --version`), how you installed it and what you did. If you can, run `RUST_LOG=info lumen-rs` from a terminal and attach the log. For a problem with one file, say what kind of file it is and attach it if it is not private. If a picture will not open, try `LUMEN_NO_SANDBOX=1` and say whether that changes anything.

## Setting up

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for what you need, how to run the tests and how to check changes to the interface, and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the code is laid out. In short:

```sh
cargo run --release -- path/to/file.pdf
```

## Before you send a pull request

CI runs these, so run them first:

```sh
make test     # cargo test --locked
make lint     # cargo fmt --check, then clippy with warnings as errors
```

Guidelines:

- **Pure Rust, no C in the document path.** New dependencies must be pure Rust; the app is `#![forbid(unsafe_code)]`. Anything that needs a C library goes behind an optional feature and needs discussing first.
- Keep logic out of widgets. Geometry, edit operations, the annotation model and PDF reading and writing live in plain modules with unit tests; the message flow is tested in `src/app/tests.rs`.
- Never block the interface. Anything that touches the disk, decodes or renders runs on a worker thread or in the decoder process.
- New interface follows `src/style.rs` (one surface, floating panels, the amber accent only for what is active) and uses `icon_button` and `tip` so every icon has a tooltip.
- Visible text goes through `tr()` (or `trf!()` with `{}` holes) and needs an entry in `po/es.po`; `cargo test` tells you which are missing.
- One focused change per pull request, with a message that says what and why. Update `docs/FEATURES.md` for a new feature and `CHANGELOG.md` under `Unreleased`.

## Translations

Copy `po/es.po` to `po/<code>.po`, translate the `msgstr` lines, and add one line to `CATALOGUES` in `src/i18n.rs`. Keep the `{}` holes in the same number and order. `po/es.po` is a complete example.

## Licence

By contributing you agree that your work is released under [GPL-3.0-or-later](LICENSE).
