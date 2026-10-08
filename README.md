# ck

[![crates.io](https://img.shields.io/crates/v/ck.svg)](https://crates.io/crates/ck)
[![downloads](https://img.shields.io/crates/d/ck.svg)](https://crates.io/crates/ck)
[![license](https://img.shields.io/crates/l/ck.svg)](#license)
[![CI](https://github.com/gold-silver-copper/ck/actions/workflows/ci.yml/badge.svg)](https://github.com/gold-silver-copper/ck/actions/workflows/ci.yml)
[![Built With Ratatui](https://ratatui.rs/built-with-ratatui/badge.svg)](https://ratatui.rs/)

[![A two-minute tour of ck](ck-demo.avif)](ck-demo.webm)

## Install

    cargo install ck

Run `ck`, or `ck 4chan/g` (or any thread or board URL) to start there.

A read-only terminal browser for imageboards, built with [ratatui](https://ratatui.rs): 4chan,
vichan and LynxChan sites, jschan, 2ch and FoolFuuka archives, with images drawn in the
terminal. (Click the tour for [the video](ck-demo.webm), with controls.)

## Features

- **Catalogs** as cards, a list or a thumbnail grid, marking what's new since your last visit.
- **Threads** with quote previews, replies shown inline, one post's conversation alone, and search.
- **Images** in the terminal (kitty, sixel, iTerm2, or half-blocks anywhere), a full-screen
  viewer that plays GIFs, a gallery of a thread's files, downloads and reverse image search.
- **Watched threads** refresh in the background and notify you of new posts and replies to
  yours. They're saved as they go, so a thread that dies can still be read offline.
- **Filters** and hidden words hide or highlight threads and posts.
- **Archive search** on FoolFuuka archives, and the archive's copy of a thread that 404'd.
- **Tabs**, `:` to open any thread or board URL, and the session restored at the next start.
- **Themes**, remappable keys, and a settings screen that writes your config.

ck follows 4chan's API rules on every site: at most one request a second per host, and
no refetching what hasn't changed.

## Sites

| kind        | engine                                         | built in                         |
|-------------|------------------------------------------------|----------------------------------|
| `4chan`     | the official 4chan API                         | 4chan                            |
| `vichan`    | vichan, tinyboard, infinity                    | lainchan, wizchan, uboachan, smuglo.li, kissu, tvch, sushigirl, leftypol, 8kun |
| `lynxchan`  | LynxChan                                       | endchan, kohlchan, alogs         |
| `jschan`    | jschan                                         | zzzchan, trashchan, ptchan, erischan, junkuchan, nukechan |
| `makaba`    | 2ch.hk's engine                                | 2ch                              |
| `foolfuuka` | FoolFuuka archives                             | desuarchive, palanq, b4k         |

To add another site running one of these, paste a link to it after `:`.

## Keys

You need few. With something selected:

- `.` (or right-click) lists what you can do with it, each with its key;
- `tab` steps through a post's images, links and replies, and `enter` opens one;
- `f` labels everything on screen: type a label to open it;
- `?` lists every key, starting with the ones for where you are.

| key | action | key | action |
|-----|--------|-----|--------|
| `j`/`k`, `g`/`G` | move, top / bottom | `v` / `V` | image viewer / gallery |
| `enter`, `esc` | open, back | `w` | watch a thread |
| `:` | go to a URL or `site/board/thread` | `T`, `]` / `[` | new tab, next / previous |
| `/` | filter the list, search a thread | `H` / `Z` / `X` | hide, show hidden, make a filter |
| `c` | layout (catalog), conversation (thread) | `,` | settings |
| `s` | sort (catalog) | `q` | quit |

Every key can be remapped, in Settings or the config.

## Configuration

Everything is optional, and Settings (`,`) writes the config for you. To start from the
default, with every option explained:

    mkdir -p ~/.config/ck && ck --print-config > ~/.config/ck/config.toml

`ck --help` shows where the config, data and caches live. The [manual](docs/manual.md)
covers every key, option and feature in detail.

## Development

    cargo test                                        # offline: unit, snapshot and short fuzz tests
    cargo test -- --ignored live --nocapture          # every built-in site, live (rate-limited)

The manual's [Tests](docs/manual.md#tests) section covers the snapshots, fuzzers and
end-to-end tests; `demo/record.sh` records the video above.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
