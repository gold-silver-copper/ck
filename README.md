# ck

A terminal imageboard browser built with [ratatui](https://ratatui.rs). One interface for 4chan, lainchan, and any other chan running a supported engine:

| kind       | engines                                  | examples                         |
|------------|------------------------------------------|----------------------------------|
| `4chan`    | the official 4chan API                   | 4chan                            |
| `vichan`   | vichan, tinyboard, infinity (4chan-style JSON) | lainchan, wizchan, uboachan |
| `lynxchan` | LynxChan                                 | endchan, kohlchan                |
| `foolfuuka`| FoolFuuka 4chan archives                 | desuarchive, b4k                 |
| `jschan`   | jschan                                   | zzzchan                          |

Read-only: browse boards, catalogs, and threads. Files open in your default viewer/browser.

ck follows 4chan's API rules on every site: at most one API request per second per host,
If-Modified-Since on every refetch, and no refetching the same page within 10 seconds
(a reload inside that window says "Up to date" and uses the cached copy).

## Run

    cargo run --release

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

Both lists are stored as JSON in `$XDG_DATA_HOME/ck` (default `~/.local/share/ck`).

## Images

Catalog and thread views show thumbnails, and `v` opens a full-screen viewer. ck asks the
terminal which image protocol it supports (kitty, sixel, iTerm2) and falls back to unicode
half-blocks, which work everywhere. The detected protocol is shown at the bottom of the `?`
help. Thumbnails are skipped in terminals narrower than 60 columns.

Images load in the background through the same rate limiter as everything else, only for
what's on screen (or about to be), and are kept in a bounded in-memory cache.

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

    cargo test                           # offline unit tests
    cargo test -- --ignored --nocapture  # hit every default site live
