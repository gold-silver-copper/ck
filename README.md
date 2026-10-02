# ck

A terminal imageboard browser built with [ratatui](https://ratatui.rs). One interface for 4chan, lainchan, and any other chan running a supported engine:

| kind       | engines                                  | examples                         |
|------------|------------------------------------------|----------------------------------|
| `4chan`    | the official 4chan API                   | 4chan                            |
| `vichan`   | vichan, tinyboard, infinity (4chan-style JSON) | lainchan, wizchan, uboachan, smuglo.li, kissu, tvch, sushigirl |
| `lynxchan` | LynxChan                                 | endchan, kohlchan, alogs         |
| `foolfuuka`| FoolFuuka 4chan archives                 | desuarchive, palanq, b4k         |
| `jschan`   | jschan                                   | zzzchan, trashchan, ptchan, erischan, junkuchan, nukechan |

All of the examples are in the default config, so they show up without any setup.

Read-only: browse boards, catalogs, and threads. Files open in your default viewer/browser.

ck follows 4chan's API rules on every site: at most one API request per second per host,
If-Modified-Since on every refetch, and no refetching the same page within 10 seconds
(a reload inside that window says "Up to date" and uses the cached copy).

## Install

    cargo install ck

Or from a checkout: `cargo run --release`. `ck --help` shows where the config, data and
thumbnail cache live.

## Keys

Navigation (fixed):

| key                     | action                                        |
|-------------------------|-----------------------------------------------|
| `j`/`k`, arrows         | move (in a thread: next/previous post)        |
| `g`/`G`                 | top / bottom                                  |
| ctrl-d / ctrl-u, space  | page                                          |
| `J`/`K`                 | scroll a thread by line                       |
| `enter`, `l`            | open (in a thread: follow a `>>quote`, also into other threads and boards) |
| `esc`, `h`, backspace   | back                                          |
| mouse                   | wheel scrolls, click selects, double-click opens |

Commands (remappable, see [Configuration](#configuration); the action name is in brackets):

| key  | where                 | action                                                     |
|------|-----------------------|------------------------------------------------------------|
| `/`  | everywhere            | filter the list; in a thread, search it [`search`]         |
| `r`  | everywhere            | reload [`reload`]                                          |
| `o`  | everywhere            | open the board/thread in a browser [`browser`]             |
| `?`  | everywhere            | help [`help`]                                              |
| `q`  | everywhere            | quit [`quit`]                                              |
| `v`  | catalog, thread       | image viewer for the post's files (catalog: the OP's) [`view`] |
| `w`  | catalog, thread       | watch / unwatch the thread [`watch`]                       |
| `s`  | catalog               | cycle the sort: bump order, most replies, newest, oldest [`sort`] |
| `c`  | catalog               | compact layout, one line per thread [`compact`]            |
| `p`  | thread                | preview the posts a post quotes [`preview`]                |
| `b`  | thread                | jump to the first reply [`replies`]                        |
| `u`  | thread                | jump back, also to the previous thread [`jump_back`]       |
| `n`/`N` | thread             | next / previous search match [`next_match`, `prev_match`]  |
| `s`/`S` | thread             | show spoilers in the post / whole thread [`spoiler`, `all_spoilers`] |
| `U`  | thread                | jump to the first unread post [`unread`]                   |
| `i`  | thread                | open the post's file (videos in mpv if it's installed) [`open_file`] |
| `d`/`D` | thread             | save the post's / whole thread's files [`download`, `download_thread`] |
| `a`  | thread                | after a 404: open the thread in the site's archive [`archive`] |
| `x`  | Watched, History      | remove the entry [`remove`]                                |

In the image viewer: `h`/`l` or arrows for the previous/next file, `i` to open it externally,
`esc` or `q` to close.

## Watched threads and history

`w` watches the open thread, or the selected one in a catalog. The "Watched" entry at the
top of the Sites view lists watched threads from all sites with their post counts and how
many posts are new; threads that 404 stay listed as "archived/deleted". "History" lists the
last 100 threads you opened. `x` removes an entry from either list.

The open thread refreshes in the background every 10 seconds and watched threads every
60 seconds (change with `refresh_thread_secs` / `refresh_watched_secs`; those are also the
minimums). Posts that arrived since your last visit are marked "● new"; `U` jumps to the
first one.

Both lists are stored as JSON in `$XDG_DATA_HOME/ck` (default `~/.local/share/ck`). Board
lists fetched from sites are saved there too (`boards/`), so a site's boards show up
instantly next time; they're refreshed quietly in the background once a day, and `r` in
the Boards view refreshes them now. Lists that come in several pages (LynxChan and jschan
board lists, FoolFuuka catalogs) show each page as it arrives.

## Images

Catalog and thread views show thumbnails, and `v` opens a full-screen viewer. ck asks the
terminal which image protocol it supports (kitty, sixel, iTerm2) and falls back to unicode
half-blocks, which work everywhere. The detected protocol is shown at the bottom of the `?`
help. Thumbnails are skipped in terminals narrower than 60 columns.

Images load in the background through the same rate limiter as everything else, only for
what's on screen (or about to be), and are kept in a bounded in-memory cache. Thumbnails
are also cached on disk in `$XDG_CACHE_HOME/ck/thumbs` (default `~/.cache/ck/thumbs`, at
most 200 MB, least recently used first out), so revisiting a catalog or thread shows them
at once without any requests. On sites whose images share the page's rate limit, visible
thumbnails load top to bottom and nothing is prefetched ahead of them.

To turn images off entirely (no image requests at all), put `images = "off"` at the top of
your config.

## Downloads

`d` saves the selected post's files and `D` all of the thread's, into
`~/Downloads/ck/{site}/{board}/{thread}/` (your system's Downloads folder). Files are named
`{post}_{original name}` with unsafe characters replaced; files that already exist are
skipped, so `D` again later only fetches what's new. Progress shows at the right of the
footer. Downloads go through the same rate limiter as images.

## Configuration

    mkdir -p ~/.config/ck && ck --print-config > ~/.config/ck/config.toml

All settings are optional; see `config.example.toml` for every option with comments:

- `images = "auto" | "off"`
- `refresh_thread_secs`, `refresh_watched_secs`
- `compact_catalog = true` (also toggled with `c`, which saves it here, comments intact)
- `download_dir = "~/stuff/{site}/{board}/{thread}"`
- `[keys]`: `action = "key"`, e.g. `watch = "W"`. Unknown actions, keys that aren't a single
  character, and two commands on one key in the same view are reported at startup.
- `[theme]`: `accent`, `dim`, `selected`, `search`, `name`, `greentext`, `quotelink`,
  `heading`, `code`, `new`. Colors are names (`"light-blue"`), `"#rrggbb"`, or 256-color
  indexes (`"244"`).

## Adding sites

Add a `[[site]]` entry. vichan sites have no board-list API, so they need an explicit `boards` list:

```toml
[[site]]
name = "somechan"
kind = "vichan"
url = "https://somechan.org"
boards = ["b", { uri = "tech", title = "Technology" }]
thumb_ext = "png"   # only if the site renders every thumbnail as png (vichan's `thumb_ext`)
```

A site can name a FoolFuuka archive with `archive = "desuarchive"`. When one of its
threads 404s, ck offers to open it there (`a`).

## Tests

    cargo test                           # offline: unit, parsing and snapshot tests
    cargo test -- --ignored --nocapture  # hit every default site live (rate-limited)

The offline tests parse real, trimmed responses from every engine in `tests/fixtures/`, and
render every view with fixed data and a fixed clock into the snapshots in `src/snapshots/`
([insta](https://insta.rs)). After an intended UI change, review the differences and update
them with `INSTA_UPDATE=always cargo test` (or `cargo insta review`).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
