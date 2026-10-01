# ck

A terminal imageboard browser built with [ratatui](https://ratatui.rs). One interface for 4chan, lainchan, and any other chan running a supported engine:

| kind       | engines                                  | examples                         |
|------------|------------------------------------------|----------------------------------|
| `4chan`    | the official 4chan API                   | 4chan                            |
| `vichan`   | vichan, tinyboard, infinity (4chan-style JSON) | lainchan, wizchan, uboachan |
| `lynxchan` | LynxChan                                 | endchan, kohlchan                |

Read-only: browse boards, catalogs, and threads. Files open in your default viewer/browser.

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
| `i`                    | open the post's file                 |
| `o`                    | open the board/thread in a browser   |
| `r`                    | reload                               |
| `?`                    | help                                 |
| `q`                    | quit                                 |

## Adding sites

    mkdir -p ~/.config/ck && ck --print-config > ~/.config/ck/config.toml

Then add a `[[site]]` entry. vichan sites have no board-list API, so they need an explicit `boards` list:

```toml
[[site]]
name = "somechan"
kind = "vichan"
url = "https://somechan.org"
boards = ["b", { uri = "tech", title = "Technology" }]
```

## Tests

    cargo test                           # offline unit tests
    cargo test -- --ignored --nocapture  # hit every default site live
