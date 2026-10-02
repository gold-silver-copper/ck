# ck

A terminal imageboard browser built with [ratatui](https://ratatui.rs). One interface for 4chan, lainchan, and any other chan running a supported engine:

| kind       | engines                                  | examples                         |
|------------|------------------------------------------|----------------------------------|
| `4chan`    | the official 4chan API                   | 4chan                            |
| `vichan`   | vichan, tinyboard, infinity (4chan-style JSON) | lainchan, wizchan, uboachan, smuglo.li, kissu, tvch, sushigirl, leftypol, 8kun |
| `lynxchan` | LynxChan                                 | endchan, kohlchan, alogs         |
| `foolfuuka`| FoolFuuka 4chan archives                 | desuarchive, palanq, b4k         |
| `jschan`   | jschan                                   | zzzchan, trashchan, ptchan, erischan, junkuchan, nukechan |
| `makaba`   | 2ch.hk's engine                          | 2ch                              |

All of the examples are in the default config, so they show up without any setup.

Overboards (one catalog mixing threads from many boards) work where the site says which
board each thread is on: jschan and LynxChan sites list theirs first in the Boards view,
and leftypol's are in the default config. Catalog entries from another board are tagged
with it, threads open on their own board, and going back returns to the overboard.

Read-only: browse boards, catalogs, and threads. Files open in your default viewer/browser.

What it does, besides browsing:

- **Images** in catalogs and threads, a full-screen viewer (animated GIFs play), a gallery
  of a thread's files, downloads, and reverse image search.
- **Tabs**, and **`:`** to open any thread or board URL; `ck URL` starts there, and ck
  otherwise starts where you left off.
- **Watched threads** refreshed in the background, with desktop notifications for new posts
  and replies to posts you've marked as yours.
- **Catalogs** as cards, a compact list or a grid, marking threads that are new since your
  last visit; **filters** to hide or highlight threads and posts.
- **Threads** with replies expandable inline, quote previews, search, a links panel, and
  copying text and links to the clipboard; save a thread as a page for offline reading.
- **Archive search** on FoolFuuka archives (desuarchive and others).
- **Themes**, all **keys remappable**, and a settings screen that edits the config file.

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

Commands are remappable in Settings or the config (see [Remapping keys](#remapping-keys));
the action name is in brackets. The `?` help lists the same, with your keys.

**Everywhere**

| key  | action |
|------|--------|
| `/`  | filter the list; in a thread, search it [`search`] |
| `r` / `o` | reload / open the board or thread in a browser [`reload`, `browser`] |
| `:`  | go to a URL or a site/board/thread, see [Going to a URL](#going-to-a-url) [`goto`] |
| `,`  | settings: theme, colors, keys, and more [`settings`] |
| tab / shift-tab | next / previous tab [`next_tab`, `prev_tab`] |
| ctrl-w | close the tab [`close_tab`] |
| `?`  | help [`help`] |
| `q`, ctrl-c | quit [`quit`] |

**Home screen, Boards**

| key  | action |
|------|--------|
| `1`–`9` | open a favorite board (home screen) |
| `*`  | favorite board on / off (also in a catalog) [`favorite`] |
| `x`  | on the home screen: take a favorite off, forget a recent board, hide / show a site [`remove`] |

**Watched, History**

| key  | action |
|------|--------|
| `x`  | remove the entry [`remove`] |
| `T`  | open the thread in a new tab [`new_tab`] |
| `y` / `Y` | copy the subject and link / the link [`copy`, `copy_link`] |

**Image viewer**

| key  | action |
|------|--------|
| `h`/`l`, arrows | previous / next file (animated GIFs play) |
| space | pause an animated GIF |
| `i`  | open the file externally |
| `y` / `Y` | copy the file's URL / the post's link [`copy`, `copy_link`] |
| `R`  | reverse image search [`image_search`] |
| `esc`, `q` | close |

**Catalog**

| key  | action |
|------|--------|
| `v`  | image viewer for the OP's files [`view`] |
| `w` / `T` | watch / unwatch the thread; open it in a new tab [`watch`, `new_tab`] |
| `s`  | cycle the sort: bump order, most replies, newest, oldest (remembered per board) [`sort`] |
| `c`  | cycle the layout: cards, compact (a line per thread), grid (thumbnails in columns), remembered per board [`compact`] |
| `O`  | the OP's links and files [`links`] |
| `H` / `Z` | hide / unhide the thread; show hidden threads, dimmed [`hide`, `show_hidden`] |
| `f`  | search the board's archive [`archive_search`] |
| `y` / `Y` | copy the OP's subject and text / the thread's link [`copy`, `copy_link`] |

**Thread**

| key  | action |
|------|--------|
| `J`/`K`, space | scroll by line / page |
| `enter`, `l` | follow a `>>quote`, also into other threads and boards |
| `p` / `b` | preview the posts a post quotes / jump to the first reply [`preview`, `replies`] |
| `u` / `U` | jump back (also to the previous thread) / to the first unread post [`jump_back`, `unread`] |
| `n` / `N` | next / previous search match [`next_match`, `prev_match`] |
| `s` / `S` | show spoilers in the post / the whole thread [`spoiler`, `all_spoilers`] |
| `e`  | show / hide the post's replies under it, indented; again on a reply goes a level deeper (up to 4) [`expand`] |
| `v` / `V` | image viewer for the post's files / gallery of every file in the thread [`view`, `gallery`] |
| `i` / `R` | open the post's file (videos in mpv if it's installed) / reverse image search [`open_file`, `image_search`] |
| `O`  | the post's links: quotes of other threads, web links, files; `enter` opens, `y` copies [`links`] |
| `d` / `D` / `E` | save the post's files / all the thread's files / the thread as a page [`download`, `download_thread`, `export`] |
| `w` / `T` | watch / unwatch; follow the quote `enter` would follow in a new tab [`watch`, `new_tab`] |
| `H` / `Z` | hide / unhide the post; show hidden posts [`hide`, `show_hidden`] |
| `m`  | mark the post as yours, to be told about replies [`mine`] |
| `a`  | after a 404: open the thread in the site's archive [`archive`] |
| `y` / `Y` | copy the post's text / its link [`copy`, `copy_link`] |

In the gallery (`V`), `h`/`j`/`k`/`l` move, `enter` views (`h`/`l` there go through every
file of the thread), `d` saves the file, `esc` returns to its post.

Copying uses the terminal's clipboard escape (OSC 52), which also works over SSH; in tmux it
needs `set -g set-clipboard on`. On a local machine ck also uses `pbcopy`, `wl-copy`,
`xclip` or `xsel` when one is installed.

### The home screen

`ck` starts on the home screen: Watched and History, your favorite boards, then the sites.
`*` on a board (in the Boards list, or in its catalog) makes it a favorite; favorites are
listed at the top, and `1`–`9` open the first nine directly. `x` on a favorite takes it
off. They're kept in the config as `favorites = ["4chan/g", "lainchan/λ"]`.

Under them, marked ↺, are the last five boards you opened (that aren't favorites); `x`
forgets one. The list is kept in the data directory (`recent_boards.json`).

`x` on a site hides it from the home screen (`hidden_sites = [...]` in the config). The
last row says how many are hidden; `enter` on it shows them (marked "hidden"), and `x` on
one brings it back.

### Tabs

`T` opens the selected thread in a new tab, next to the current one (in a thread, the
quoted thread `enter` would go to). Each tab has its own place: view, board, catalog,
thread, and jump trail. The tabs show as chips under the top bar when there's more than
one; `tab` and `shift-tab` move between them (or click one), `ctrl-w` closes one (not
the last). Up to 9. Only the tab on screen refreshes its thread in the background;
another tab's thread is refreshed when you come back to it (watched threads are refreshed
anyway). The tabs are part of the session that's restored at the next start.

### Going to a URL

`:` asks where to go. Paste a thread or board URL from any configured site (pasting while
nothing else is being typed starts this by itself), or type a short form:

| input                  | goes to                                            |
|------------------------|----------------------------------------------------|
| `g`, `/g/`             | /g/ on the current site                            |
| `123`                  | thread 123 on the current board                    |
| `4chan`                | 4chan's board list                                 |
| `4chan/g`, `4chan/g/123` | a board, a thread                                |
| `lainchan/λ/42#43`     | a thread, with post 43 selected                    |
| `>>>/g/123`            | a cross-board quote: thread 123 on /g/             |

`tab` completes site and board names. `esc` (or `u` from a thread) goes back to where you
were. `ck URL` (or `ck 4chan/g`) starts there.

### Remapping keys

`,` → Key bindings lists every command with its keys. `enter` and then a key rebinds the
command, `a` adds another key, `x` resets it to the default. A key that another command
already uses in the same view is refused. Changes apply at once and are saved in the
config's `[keys]` section (only the keys you changed):

```toml
[keys]
watch = "W"
help = ["?", "f1"]
reload = "ctrl-r"
```

Keys are a character (`"w"`, `"W"`, `":"`), `ctrl-` or `alt-` with one, or a named key:
`tab`, `shift-tab`, `enter`, `esc`, `backspace`, `delete`, `insert`, arrows (`up`, ...),
`home`, `end`, `pageup`, `pagedown`, `space`, `f1`–`f12`. Navigation keys (the first
table) are fixed, as are keys inside text inputs and popups. In the image viewer only
the viewer's own commands apply.

## Filters and hiding

`H` hides the selected catalog thread or thread post; `Z` shows hidden ones again
(dimmed and marked) so they can be unhidden with another `H`. Hidden posts collapse to
one line, so replies to them still make sense. What you hide is remembered per board in
the data directory.

Filters in the config hide or highlight automatically:

```toml
[[filter]]
pattern = "(?i)crypto|nft"     # a regex; (?i) for any case
label = "crypto"               # shown on what it hides or highlights

[[filter]]
pattern = "(?i)rust"
action = "highlight"           # "hide" (the default) or "highlight"
field = ["subject", "comment"] # subject, comment, name, filename, md5; default subject + comment
sites = ["4chan"]              # optional: only these sites
boards = ["g"]                 #           and these boards

[[filter]]
pattern = "u8Vh17KxaDvUJ6bBcmE/eg=="   # field = "md5": a file's MD5 (base64, as 4chan shows it)
field = "md5"
```

Highlighted threads and posts get the label as a chip and an accent stripe; the catalog's
header says how many are hidden. A bad pattern is reported at startup with its filter's
number.

## Watched threads and history

`w` watches the open thread, or the selected one in a catalog. The "Watched" entry at the
top of the Sites view lists watched threads from all sites with their post counts and how
many posts are new; threads that 404 stay listed as "archived/deleted". "History" lists the
last 100 threads you opened. `x` removes an entry from either list.

The open thread refreshes in the background every 10 seconds and watched threads every
60 seconds (change with `refresh_thread_secs` / `refresh_watched_secs`; those are also the
minimums). Posts that arrived since your last visit are marked "new"; `U` jumps to the
first one.

### What's new in a catalog

Catalogs mark threads that weren't there on your previous visit with "new" (the header
counts them), and threads you've opened before show how many replies they've gained since,
like `+12`. This is kept per board in the data directory (threads are forgotten a week
after they leave the catalog) and needs no extra requests.

### Notifications

When a background refresh finds new posts in a watched thread (other than the one on
screen), ck tells you: with a desktop notification in terminals that show them (iTerm2,
kitty, WezTerm, Ghostty, Windows Terminal via OSC 9; foot and urxvt via OSC 777), and
with the terminal bell elsewhere, including inside tmux and screen. News from several
threads at once makes one notification. `notify = "bell"` always rings the bell,
`notify = "off"` stays quiet (also in Settings), and `notify_command` runs a program
instead (no shell; `{title}` and `{body}` are filled in):

```toml
notify_command = ["notify-send", "{title}", "{body}"]
```

ck never posts, so it can't know which posts are yours: `m` marks the selected post as
yours (and watches the thread). Replies to it are counted in Watched ("1 reply to you"),
quotes of it read `>>123 (You)`, and they get their own notification.

ck keeps its state as JSON in `$XDG_DATA_HOME/ck` (default `~/.local/share/ck`):
`watched.json` and `history.json`, `recent_boards.json`, `board_prefs.json` (each board's
sort and layout), `hidden.json` (what you hid with `H`), `seen.json`
(catalog threads seen, for "new" and `+N`), and `session.json` (your tabs, for the next
start). Board lists fetched from sites are saved there too (`boards/`), so a site's boards
show up instantly next time; they're refreshed quietly in the background once a day, and
`r` in the Boards view refreshes them now. Lists that come in several pages (LynxChan and jschan
board lists, FoolFuuka catalogs) show each page as it arrives.

## Searching archives

`f` in a catalog searches the board's posts on a FoolFuuka archive: the site itself if
it's one (desuarchive, palanq, b4k), or the archive configured for it with `archive`
(4chan's boards use desuarchive in the default config). Results show each post with its
thread; `enter` opens the thread on the archive with the post selected, `esc` goes back.
Each page of 25 results is one request; going down past the last one (or `n`) loads the
next. Archives limit how often you can search; when they say no, ck shows their message.

## Images

Catalog and thread views show thumbnails, and `v` opens a full-screen viewer. ck asks the
terminal which image protocol it supports (kitty, sixel, iTerm2) and falls back to unicode
half-blocks, which work everywhere. The detected protocol is shown at the bottom of the `?`
help. Thumbnails are skipped in terminals narrower than 60 columns.

Animated GIFs play in the viewer, at up to 20 frames a second (frames are prepared in the
background, and scaled down if a long GIF would take too much memory). Videos open
externally with `i`.

Images load in the background through the same rate limiter as everything else, only for
what's on screen (or about to be), and are kept in a bounded in-memory cache. Thumbnails
are also cached on disk in `$XDG_CACHE_HOME/ck/thumbs` (default `~/.cache/ck/thumbs`, at
most 200 MB, least recently used first out), so revisiting a catalog or thread shows them
at once without any requests. On sites whose images share the page's rate limit, visible
thumbnails load top to bottom and nothing is prefetched ahead of them.

To turn images off entirely (no image requests at all), put `images = "off"` at the top of
your config.

### Reverse image search

`R` lists search engines for the selected post's images (for videos, their thumbnail) or
the image in the viewer; `enter` opens the search in the browser, `y` copies its link.
ck itself sends nothing anywhere. The engines can be replaced in the config; `{url}`
becomes the image's address:

```toml
[[image_search]]
name = "SauceNAO"
url = "https://saucenao.com/search.php?url={url}"
```

## Downloads

`d` saves the selected post's files and `D` all of the thread's, into
`~/Downloads/ck/{site}/{board}/{thread}/` (your system's Downloads folder). Files are named
`{post}_{original name}` with unsafe characters replaced; files that already exist are
skipped, so `D` again later only fetches what's new. Progress shows at the right of the
footer. Downloads go through the same rate limiter as images.

### Saving a thread

`E` writes the open thread into its download folder (the same one `D` uses) as
`thread.html`, a page with the current theme's colors that reads offline, and
`thread.json`, the posts as data (`"format": 1`). Files already saved there with `d`/`D`
are shown from the folder; the others link to the site. So `D` then `E` makes a complete
offline copy. Saving again replaces both.

## Themes and settings

ck draws flat: no lines or boxes, just areas of color. The screen is the darkest tone,
catalog entries and posts are cards a step lighter, and popups another step up; the
selected row, card or post is tinted and marked by a stripe in the theme's accent color.

`,` opens Settings. Its options are saved to your config file, keeping your comments,
and the file is created from the default if you don't have one yet:

- **Theme**: pick from the built-in themes (material, material-light, nord, gruvbox,
  catppuccin, tokyo-night, solarized-light, and terminal, which uses your terminal's own
  colors) and your own. The screen changes as you move through the list; `enter` keeps
  the theme, `esc` goes back.
- **Colors**: every color the theme uses, with a swatch and what it's for. `enter` edits
  one (`#rrggbb`, a name, or a 256-color index), `x` resets it. Changing a built-in theme
  saves your changes as a copy, `[themes.NAME-custom]`.
- Color depth, the catalog layout, images, the refresh intervals, the download folder,
  and key bindings (see [Remapping keys](#remapping-keys)).

Custom themes go in the config as `[themes.NAME]` tables. Start from a built-in theme and
change some colors, or generate a whole theme from one color:

```toml
theme = "ocean"

[themes.mine]
base = "nord"
primary = "#ebcb8b"

[themes.ocean]
seed = "#2a9d8f"   # tones for every role are derived from this
mode = "dark"      # or "light"
```

The roles are listed at the end of `config.example.toml`. Terminals that don't support
24-bit color (when `COLORTERM` isn't `truecolor`) get the nearest of 256 colors; set
`color = "truecolor"` or `"256"` to choose.

## Configuration

    mkdir -p ~/.config/ck && ck --print-config > ~/.config/ck/config.toml

All settings are optional; see `config.example.toml` for every option with comments:

- `images = "auto" | "off"`
- `refresh_thread_secs`, `refresh_watched_secs`
- `catalog_layout = "cards" | "compact" | "grid"`: the default layout (also in Settings).
  `c` in a catalog and `s` set that board's own layout and sort, which are remembered in
  the data directory (`board_prefs.json`). In the grid, `h`/`l` move between columns (`h` in the first
  column goes back) and `j`/`k` between rows; without images it's shown as cards.
- `download_dir = "~/stuff/{site}/{board}/{thread}"`
- `restore_session = false` to start at the site list instead of where you left off (the
  view, thread and selected post, catalog sort and filter, saved in the data directory
  as `session.json`). `ck URL` always starts at the URL.
- `favorites = ["4chan/g", ...]`, `hidden_sites = ["wizchan", ...]`: see
  [The home screen](#the-home-screen).
- `notify = "auto" | "bell" | "off"`, `notify_command = [...]`: see
  [Notifications](#notifications).
- `[[filter]]`: hide or highlight threads and posts, see [Filters](#filters-and-hiding).
- `[[image_search]]`: reverse image search engines, see
  [Reverse image search](#reverse-image-search).
- `archive = "desuarchive"` on a `[[site]]`: the archive for 404'd threads and `f` searches.
- `[keys]`: `action = "key"` or `action = ["key", ...]`, e.g. `watch = "W"`; see
  [Remapping keys](#remapping-keys). Unknown actions, things that aren't keys, and two
  commands on one key in the same view are reported at startup.
- `theme = "nord"`, `[themes.NAME]`, `color = "auto" | "truecolor" | "256"`: see
  [Themes and settings](#themes-and-settings). A ck 0.2 `[theme]` table of colors still
  works, on top of the default theme.

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
