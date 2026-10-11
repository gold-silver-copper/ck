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

A terminal browser for imageboards, built with [ratatui](https://ratatui.rs): 4chan,
vichan and LynxChan sites, jschan, 2ch and FoolFuuka archives, with images drawn in the
terminal. (Click the tour for [the video](ck-demo.webm), with controls.)

## Features

- **Catalogs** as cards, a list or a thumbnail grid, marking what's new since your last visit.
- **Threads** with quote previews, replies shown inline, one post's conversation alone, and search.
- **Images** in the terminal (kitty, sixel, iTerm2, or half-blocks anywhere), a full-screen
  viewer that plays GIFs, a gallery of a thread's files, downloads and reverse image search.
- **Posting** on 4chan, vichan, LynxChan, jschan and 2ch sites: a reply box with quoting and
  drafts, the site's captcha answered in the terminal, and Cloudflare's check clicked there
  too; your posts are marked as yours. The first post downloads ck-web, the browser ck posts
  through (about 135 MB, once; Linux for now).
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
| `vichan`    | vichan, tinyboard, infinity                    | lainchan, wizchan, uboachan, smuglo.li, tvch, sushigirl, leftypol, 8kun |
| `kissu`     | kissu's own engine (read as vichan)            | kissu                            |
| `lynxchan`  | LynxChan                                       | endchan, kohlchan, alogs         |
| `jschan`    | jschan                                         | zzzchan, trashchan, ptchan, erischan, junkuchan, nukechan |
| `makaba`    | 2ch.hk's engine                                | 2ch                              |
| `foolfuuka` | FoolFuuka archives                             | desuarchive, palanq, b4k         |

To add another site running one of these, paste a link to it after `:`.

## Keys

You need few. With something selected:

- `enter` does the obvious thing: opens a thread, shows a post's images, follows a quote;
- `h` or `esc` goes back, like a browser's back button;
- `.` (or right-click) lists everything else you can do with it, the less used on a letter
  there (`.` then `H` hides a post);
- `c` changes what's shown: a catalog's sort and layout, a thread's conversation or
  poster's posts;
- `tab` steps through a post's images, links and replies;
- `f` labels everything on screen: type a label to open it;
- `?` lists every key, starting with the ones for where you are.

| key | action | key | action |
|-----|--------|-----|--------|
| `j`/`k`, `gg`/`G` | move (`10j`), top / bottom | `w` / `W` | watch a thread / the watched ones |
| `enter`, `h` | open, back | `r` | reply |
| `/`, `n` | filter the list, search a thread | `V` | gallery |
| `:` | go to a URL or `site/board/thread` | `d` / `y` | save / copy |
| `]` / `[` | next / previous tab | `,` | settings |
| `zz` / `zt` / `zb` | the post to the middle / top / bottom | `q` | quit |

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
