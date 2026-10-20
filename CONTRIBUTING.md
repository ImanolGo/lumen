# Contributing to Lumen

Thanks for helping. Bug reports, translations and pull requests are all welcome.

## Reporting bugs

Open an [issue](https://github.com/ImanolGo/lumen/issues) with your distro, Lumen version, how you installed it (Flatpak, package or source) and what you did. If you can, run `RUST_LOG=lumen=debug lumen-viewer` and attach the log. For a file-specific problem, say what kind of file it is and attach it if it is not private.

## Setting up

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for system dependencies and debugging tips, and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the code is laid out. In short:

```sh
cargo run -- path/to/file.pdf
```

## Before you send a pull request

CI runs these, so run them first:

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
make check    # also validates the desktop file and the metainfo
```

Guidelines:

- Keep logic out of widgets. Geometry, edit operations and the annotation model live in plain Rust modules with unit tests.
- Never block the main thread. Anything that touches the disk or decodes runs on a worker.
- User-visible text goes through `tr()` and new files that use it go in `po/POTFILES.in`. Run `./build-aux/update-pot.sh` after changing texts.
- One focused change per pull request, with a message that says what and why.

## Translations

```sh
msginit -l de -i po/lumen.pot -o po/de.po   # start a new language
```

Translate the `msgstr` lines and open a pull request. `po/es.po` is a complete example.

## Licence

By contributing you agree that your work is released under [GPL-3.0-or-later](LICENSE).
