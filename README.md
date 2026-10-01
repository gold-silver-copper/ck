# ck

A terminal imageboard browser built with [ratatui](https://ratatui.rs). One interface for 4chan, lainchan, and any other chan running a supported engine:

| kind       | engines                                  | examples                         |
|------------|------------------------------------------|----------------------------------|
| `4chan`    | the official 4chan API                   | 4chan                            |
| `vichan`   | vichan, tinyboard, infinity (4chan-style JSON) | lainchan, wizchan, uboachan |
| `lynxchan` | LynxChan                                 | endchan, kohlchan                |

Read-only: browse boards, catalogs, and threads. Files open in your default viewer/browser.

ck follows 4chan's API rules on every site: at most one API request per second per host,
If-Modified-Since on every refetch, and no refetching the same page within 10 seconds
(a reload inside that window says "Up to date" and uses the cached copy).

## Run

    cargo run --release

## Keys

| key                    | action                               |
|------------------------|--------------------------------------|
| `j`/`k`, arrows        | move (in a thread: next/prev post)   |
| `enter`, `l`           | open (in a thread: follow `>>quote`) |
| `esc`, `h`, backspace  | back                                 |
| `/`                    | filter the current list              |
| `g`/`G`                | top / bottom                         |
| ctrl-d / ctrl-u, space | page                                 |
| `J`/`K`                | scroll a thread by line              |
| `b` / `u`              | jump to first reply / jump back      |
| `i`                    | open the post's file (videos in mpv if it's installed) |
| `v`                    | image viewer for the post's files (catalog: the OP's) |
| `o`                    | open the board/thread in a browser   |
| `r`                    | reload                               |
| `?`                    | help                                 |
| `q`                    | quit                                 |

In the image viewer: `h`/`l` or arrows for the previous/next file, `i` to open it externally,
`esc` or `q` to close.

## Images

Catalog and thread views show thumbnails, and `v` opens a full-screen viewer. ck asks the
terminal which image protocol it supports (kitty, sixel, iTerm2) and falls back to unicode
half-blocks, which work everywhere. The detected protocol is shown at the bottom of the `?`
help. Thumbnails are skipped in terminals narrower than 60 columns.

Images load in the background through the same rate limiter as everything else, only for
what's on screen (or about to be), and are kept in a bounded in-memory cache.

To turn images off entirely (no image requests at all), put `images = "off"` at the top of
your config.

## Adding sites

    mkdir -p ~/.config/ck && ck --print-config > ~/.config/ck/config.toml

Then add a `[[site]]` entry. vichan sites have no board-list API, so they need an explicit `boards` list:

```toml
[[site]]
name = "somechan"
kind = "vichan"
url = "https://somechan.org"
boards = ["b", { uri = "tech", title = "Technology" }]
thumb_ext = "png"   # only if the site renders every thumbnail as png (vichan's `thumb_ext`)
```

## Tests

    cargo test                           # offline unit tests
    cargo test -- --ignored --nocapture  # hit every default site live
