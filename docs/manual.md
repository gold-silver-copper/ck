# ck manual

Everything ck does, in detail. The [README](../README.md) is the short version.

## Sites

Every site in the [README's table](../README.md#sites) is built in, so they show up without
any setup. Any other site running one of these engines can be added by pasting a link to it
(see [Adding sites](#adding-sites)).

Overboards (one catalog mixing threads from many boards) work where the site says which
board each thread is on: jschan and LynxChan sites list theirs first in the Boards view,
and leftypol's are in the default config. Catalog entries from another board are tagged
with it, threads open on their own board, and going back returns to the overboard.

ck follows 4chan's API rules on every site: at most one API request per second per host,
If-Modified-Since on every refetch, and no refetching the same page within 10 seconds
(a reload inside that window says "Up to date" and uses the cached copy).

## Keys

You don't need to know many. Select something, then:

- `.` (or right-click) lists everything you can do with it, each with its key, so the menu
  is also how you learn the shortcuts;
- in a thread, `tab` steps through the post's images, links and replies (the footer says
  what the keys do to the focused one), `enter` opens it, `esc` goes back to the post;
- `f` puts a label on everything on screen; type one to open it.

`y` copies and `o` opens in the browser whatever is selected or focused: a post, a file, a
link. `d` saves it.

Navigation (fixed):

| key                     | action                                        |
|-------------------------|-----------------------------------------------|
| `j`/`k`, arrows         | move (in a thread: next/previous post)        |
| `g`/`G`                 | top / bottom                                  |
| ctrl-d / ctrl-u, space  | page                                          |
| `J`/`K`                 | scroll a thread by line                       |
| `enter`, `l`            | open (in a thread: follow a `>>quote`, also into other threads and boards) |
| `esc`, `h`, backspace   | back                                          |
| mouse                   | wheel scrolls, click selects, double-click opens, right-click opens the menu (over popups: below) |

The mouse works on what's on top. Over a list popup (the menu, links, image search, the
filter maker's choices, a settings picker) the wheel moves its rows; over the help, the
quoted posts, the gallery and the viewer it does what the arrow keys do there; over hint
labels it closes them; over a question (save this? the add-site box) it does nothing, even
with a thread behind; and while the key editor waits for a key it does nothing at all. A
click on the menu, the links or image search picks the row under it (on a link, a double
click opens it), and anywhere else closes them; in the gallery a click selects a file and a
double click views it; a right click closes the menu; a click anywhere closes the filter
maker, the help, hint labels, the quoted posts or a question, and only the one on top: a
question over the quoted posts leaves them open. Over the other Settings popups and the
viewer a click does nothing. While a go-to or archive search is typed, the wheel scrolls a
thread behind it (over a list it does nothing) and a double click on a post opens it,
leaving the box open; while a thread search or list filter is typed, a click does nothing.

A click (left or right) lands on what the screen showed. Once that may have changed (a
key, paste, resize or wheel notch, a click that goes somewhere else, even another thread,
or opens or closes Settings over where you are, or anything arriving from a site or
background work, except an image) it does nothing until the next frame is drawn, a moment
later: click again. `f` typed in that moment finds nothing to label either. The quote peek can't be
clicked (nor does `f` label what's under it), a loading thread has no posts to click, the
quoted posts or links drawn over the tab chips cover them, and a double click is two clicks
on the same thing with none of those changes between them.

Commands are remappable in Settings or the config (see [Remapping keys](#remapping-keys));
the action name is in brackets. The `?` help lists them with your keys, starting with the
ones for where you are.

**Everywhere**

| key  | action |
|------|--------|
| `/`  | filter the list; in a thread, search it [`search`] |
| `r` / `o` | reload / open the board or thread in a browser [`reload`, `browser`] |
| `:`  | go to a URL or a site/board/thread, see [Going to a URL](#going-to-a-url) [`goto`] |
| `,`  | settings: theme, colors, keys, and more [`settings`] |
| `.`, right-click | what you can do with what's selected, with each one's key [`menu`] |
| `f`  | label what's on screen (posts, images, links, rows); type a label to open it (what it was put on, even if a refresh has moved it; if it's gone, ck says so) [`hints`] |
| `]` / `[` | next / previous tab [`next_tab`, `prev_tab`] |
| ctrl-w | close the tab [`close_tab`] |
| `?`  | help [`help`] |
| `q`, ctrl-c | quit [`quit`] |

**Home screen, Boards**

| key  | action |
|------|--------|
| `1`–`9` | open a favorite board (home screen) |
| `*`  | favorite board on / off (also in a catalog) [`favorite`] |
| `x`  | on the home screen: take a favorite off, forget a recent board, hide / show a site [`remove`] |

**Watched, History, Saved**

| key  | action |
|------|--------|
| `x`  | remove the entry; in Saved, press it twice to delete the copy [`remove`] |
| `T`  | open the thread in a new tab [`new_tab`] |
| `F`  | follow / stop following as a general [`follow`] |
| `y` / `Y` | copy the subject and link / the link [`copy`, `copy_link`] |

**Image viewer**

| key  | action |
|------|--------|
| `h`/`l`, arrows | previous / next file (animated GIFs play); from a thread, every file in it (or in the conversation shown) |
| space | pause an animated GIF |
| `+` / `-` / `0` | zoom in / out (up to 800%) / fit again; zoomed in, `h`/`j`/`k`/`l` and the arrows move around, page up / down change file, `esc` fits |
| `i`  | open the file externally (its thumbnail, when that's all there is) |
| `y` / `Y` | copy the file's URL (or its thumbnail's) / the post's link [`copy`, `copy_link`] |
| `d`  | save the file [`download`] |
| `R`  | reverse image search [`image_search`] |
| `esc`, `q` | close |

**Catalog**

| key  | action |
|------|--------|
| `v`  | image viewer for the OP's files [`view`] |
| `w` / `T` | watch / unwatch the thread; open it in a new tab [`watch`, `new_tab`] |
| `F`  | follow the thread as a general [`follow`] |
| `*`  | favorite this board on / off [`favorite`] |
| `s`  | cycle the sort: bump order, most replies, newest, oldest (remembered per board) [`sort`] |
| `c`  | cycle the layout: cards, compact (a line per thread), grid (thumbnails in columns), remembered per board [`compact`] |
| `O`  | the OP's links and files [`links`] |
| `H` / `Z` | hide / unhide the thread; show hidden threads, dimmed [`hide`, `show_hidden`] |
| `X`  | hide or highlight threads like this one: by subject, image or file name (a filter) [`filter`] |
| `A`  | search the board's archive [`archive_search`] |
| `y` / `Y` | copy the OP's subject and text / the thread's link [`copy`, `copy_link`] |

**Thread**

| key  | action |
|------|--------|
| `J`/`K`, space | scroll by line / page |
| tab / shift-tab | focus the post's next / previous poster ID, image, link or reply (then on into the next post) [`next_part`, `prev_part`] |
| `enter`, `l` | on a focused part: view the image, go to the quoted post (it shows while focused), open the link, show the replies; on the post: follow its `>>quote`, also into other threads and boards |
| `esc` | from a focused part back to the post |
| `p` / `b` | preview the posts a post quotes / jump to the first reply [`preview`, `replies`] |
| `u` / `U` | jump back (also to the previous thread) / to the first unread post [`jump_back`, `unread`] |
| `n` / `N` | next / previous search match [`next_match`, `prev_match`] |
| `s` / `S` | show spoilers in the post / the whole thread [`spoiler`, `all_spoilers`] |
| `e`  | show / hide the post's replies under it, indented; again on a reply goes a level deeper (up to 4) [`expand`] |
| `c`  | the post's conversation alone: what it replies to and the replies to it; `esc` (or `c`) shows the whole thread again [`conversation`] |
| `I`  | on a board with poster IDs: that poster's posts alone; `esc` (or `I`) shows the whole thread again [`poster`] |
| `M`  | in turn: only the posts with files (and the OP), every post with its images hidden, every post again [`media`] |
| `v` / `V` | image viewer from the post's files on through the thread's / gallery of every file in the thread [`view`, `gallery`] |
| `i` / `R` | open the post's file (videos in mpv if it's installed) / reverse image search [`open_file`, `image_search`] |
| `O`  | the post's links: quotes of other threads, web links, files; `enter` opens, `y` copies [`links`] |
| `d`  | save the focused file (`tab` to it first); in the gallery, the selected one [`download`] |
| `.` menu | save all the post's files / all the thread's files / the thread as a page; the last two ask first [`download_post`, `download_thread`, `export`: no key unless you give one] |
| `w` / `T` | watch / unwatch; follow the quote `enter` would follow in a new tab [`watch`, `new_tab`] |
| `H` / `Z` | hide / unhide the post; show hidden posts [`hide`, `show_hidden`] |
| `X`  | hide or highlight posts like this one: by poster ID, name and tripcode, flag, image, file name, or (on the OP) subject (a filter) [`filter`] |
| `m`  | mark the post as yours, to be told about replies [`mine`] |
| `F`  | follow the thread as a general: when it dies or fills up, the next one is watched [`follow`] |
| `a`  | after a 404: open the thread in the site's archive [`archive`] |
| `y` / `Y` | copy the post's text (a focused part: its URL) / the post's link [`copy`, `copy_link`] |

### Where the selected post sits

While you read down a thread with `j`, the selected post doesn't creep to the bottom edge:
once it would come within `scroll_margin` of it (a fraction of the screen, 0.3 by default),
the thread scrolls so the post starts about a third of the way down, with what came before
it above and what comes next below. Reading up with `k` does the same at the top. Jumps
(following a quote, `u`, `U`, `n`, a search hit, a hint) land there too. `scroll_margin =
0.5` keeps the selected post centered, and `0` lets it reach the edges (scrolling as little
as possible, as before); Settings › Reading position cycles through them.

### Reading the end of a thread

While you read the last post of a thread (it's selected and the end of the thread is on
screen), posts a refresh brings come into view: the first new one is selected, placed as
`j` would place it, and `j` reads on through the rest. Hidden posts are passed over, and in
a conversation only new posts that belong to it count. Reading anywhere else, nothing
moves; the top bar says how many new posts are below ("3 new, 2 below ↓"), and `U` goes to
the first. Hidden posts aren't new there either (nor in the terminal title) until `Z` shows
them. `follow_new_posts = false` (or Settings › Background refresh › Reading the end)
turns it off. A thread in another tab follows when you come back to it and it refreshes.

### Long posts

A post taller than the screen is read whole: `j` scrolls on through it, a screen at a time
(keeping two lines from the screen before), and goes to the next post once its end is on
screen. `k` reads back up through it the same way, and coming up into a long post from
below starts at its end. While a post goes on below the screen, "↓ more" shows at the
bottom right ("↑" at the top when it started above), and the top bar says which screen of
it you're on, like "No.123 (2/5)". `J`/`K`, space and the mouse wheel still scroll by lines
and pages.

### Conversations

`c` on a post shows just its conversation: the post, what it quotes in the thread (and
what those quote, on up), and the replies to it (and the replies to those, on down), in
thread order. Other replies to the posts it quotes aren't in it, and since everyone quotes
the OP, the OP's replies only count when it's the OP's conversation. Replies are indented
by how far down they are; the post itself is marked "conversation", and the top bar says
"Conversation of No.123" with how many posts. Everything works as in the whole thread:
`tab` and the menu, quotes (one to a post outside the conversation leaves it and goes
there), search (within the conversation), `e`, previews, the gallery (its files). New
replies that belong in it appear as the thread refreshes. `esc` or `c` goes back to the
whole thread, scrolled where it was, and tabs and the next start remember a conversation
that was open. At most 500 posts are shown, the nearest ones; the footer says when there
are more.

### Poster IDs and flags

Where a board gives posters IDs (4chan's /pol/, /biz/, /b/ and others, 8kun, LynxChan,
jschan, archives of those), the post's header shows the ID as a chip, with how many posts
it has in the thread: `ID:Ab3dEf+g (4)`. Its color comes from the ID, so one poster's posts
look alike (under the `mono` theme, only the text). A country or board flag shows next to
it, as its two-letter code (`US`) or, for a board's own flags, its name.

`I` on a post (or `enter` on its ID, focused with `tab`, clicked, or picked with `f`) shows
that poster's posts alone, as a conversation shows its posts: the top bar says "Posts by
ID:…", everything works as in the whole thread, and `esc` or `I` goes back. It isn't
remembered across starts (IDs are the thread's alone). Filters can catch IDs and flags too
(see Filters).

In the gallery (`V`), `h`/`j`/`k`/`l` move (the wheel a row at a time), `enter` views
(`h`/`l` there go through every file of the thread), `d` saves the file, `esc` returns to
its post.

Copying uses the terminal's clipboard escape (OSC 52), which also works over SSH; in tmux it
needs `set -g set-clipboard on`. On a local machine ck also uses `pbcopy`, `wl-copy`,
`xclip` or `xsel` when one is installed.

### Posts with files, or no images

`M` shows only the posts with files (and the OP: it's the thread), the way a conversation
shows its posts: `j`/`k` and search go through those, and the top bar says "with
files". New posts with files appear as the thread refreshes, and going to a
quoted post without one shows every post again. `M` again shows every post with its images
hidden, the way a board with images off has them ("image off" in their place, the layout
the same, nothing asked for), and the bar says "images hidden"; `M` once more shows
everything. It's for that thread, while it's open; in a conversation, the conversation
still shows all its posts. (On a board you always want without images, see Images on or
off per board.)

### The home screen

`ck` starts on the home screen: Watched, History and Saved, your favorite boards, then the
sites.
`*` on a board (in the Boards list, or in its catalog) makes it a favorite; favorites are
listed at the top, and `1`–`9` open the first nine directly. `x` on a favorite takes it
off. They're kept in the config as `favorites = ["4chan/g", "lainchan/λ"]`.

Under them, marked ↺, are the last five boards you opened (that aren't favorites); `x`
forgets one. The list is kept in the data directory (`recent_boards.json`).

A site's name longer than its column is cut with "…" (`tab` after `:` still completes it
in full).

`x` on a site hides it from the home screen (`hidden_sites = [...]` in the config). The
last row says how many are hidden; `enter` on it shows them (marked "hidden"), and `x` on
one brings it back.

### Tabs

`T` opens the selected thread in a new tab, next to the current one (in a thread, the
quoted thread `enter` would go to). Each tab has its own place: view, board, catalog,
thread, and jump trail. The tabs show as chips under the top bar when there's more than
one; `]` and `[` move between them (or click one), `ctrl-w` closes one (not
the last). A tab on a watched thread counts the posts it has gained since you read it,
like `2 Thread subject (5)`. Up to 9. Only the tab on screen refreshes its thread in the background;
another tab's thread is refreshed when you come back to it (watched threads are refreshed
anyway). The tabs are part of the session that's restored at the next start.

### Going to a URL

`:` asks where to go. Paste a thread or board URL from any of your sites (pasting while
nothing else is being typed starts this by itself), or type a short form. A site's own page
(`https://lainchan.org/index.html`) goes to its board list, and a jschan site's
`/overboard.html` to its overboard. A link to a site ck doesn't have yet offers to add it
(see [Adding sites](#adding-sites)), then goes there.

| input                  | goes to                                            |
|------------------------|----------------------------------------------------|
| `g`, `/g/`             | /g/ on the current site                            |
| `123`                  | thread 123 on the current board                    |
| `4chan`                | 4chan's board list                                 |
| `4chan/g`, `4chan/g/123` | a board, a thread                                |
| `lainchan/λ/42#43`     | a thread, with post 43 selected                    |
| `>>>/g/123`            | a cross-board quote: thread 123 on /g/             |
| `saved`, `watched`, `history` | that list                                   |

`tab` completes site and board names. `esc` (or `u` from a thread) goes back to where you
were (from Settings: to the place under them). `ck URL` (or `ck 4chan/g`) starts there.

### Remapping keys

`,` → Key bindings lists every command with its keys. `enter` and then a key rebinds the
command, `a` adds another key, `u` takes its keys away, `x` resets it to the default. A key
that another command already uses in the same view is refused. Changes apply at once and
are saved in the config's `[keys]` section (only the keys you changed):

```toml
[keys]
watch = "W"
help = ["?", "f1"]
reload = "ctrl-r"
download_thread = "D"   # no key by default
mine = []               # no key: only in the . menu
```

A command without a key is still in the `.` menu where it applies.

Keys are a character (`"w"`, `"W"`, `":"`), `ctrl-` or `alt-` with one, or a named key:
`tab`, `shift-tab`, `enter`, `esc`, `backspace`, `delete`, `insert`, arrows (`up`, ...),
`home`, `end`, `pageup`, `pagedown`, `space`, `f1`–`f12`. Navigation keys (the first
table) are fixed, as are keys inside text inputs and popups. In the image viewer only
the viewer's own commands apply.

## Filters and hiding

`H` hides the selected catalog thread or thread post; `Z` shows hidden ones again
(dimmed and marked) so they can be unhidden with another `H`. Hidden posts collapse to
one line, so replies to them still make sense. They're marked just "hidden", not why
(that would show what a filter or hidden word hides); `H` on one says why. Until `Z`
shows them, they're left out of the thread's search and gallery too (`V` and the footer
and `.` menu offer the gallery only when it has files to show; when only hidden posts have
files, `V` says so), a quote of one previews
only "hidden" (with `p` or focused), saving all the thread's files leaves theirs out, and
hidden threads aren't counted as new in the catalog's header. Hiding, `Z` and filter
changes apply to every tab at once, and Watched's counts of new posts and replies to you
(and the tabs and terminal title) follow at once for the threads refreshed since ck
started (one that 404'd stays at nothing new). What you hide is remembered per board in
the data directory.

`recursive_hiding = true` (or Settings › Hidden replies) hides the replies too: in a
thread, posts that quote a hidden post, and the posts that quote those, on down, collapse
too. `recursive = true` on a `[[filter]]` does that for
what the filter hides (also in Settings › Filters, as "Replies"). Unhiding the post they
reply to shows them again; `H` on one of them says which post that is. The OP never
counts (everyone quotes it), and catalogs aren't affected.

Filters hide or highlight automatically. The quickest way to make one is `X` (or "hide or
highlight posts like it…" in the `.` menu) on a post or catalog thread: it offers what
the post can be caught by (its poster ID, its name with the tripcode, the tripcode alone,
its flag, its image's MD5, its file's name, and an OP's subject; never the site's
anonymous name), whether to hide or highlight
(`a`), where (`s`: this board, this site, or everywhere) and a label (`e`). `enter` adds
it: it's written to the config as a `[[filter]]`, applies at once, and the footer says
what it caught; `u` as the next key takes it back (also when it couldn't be written; a left
click gives that up, the wheel or a right click doesn't).

Settings › Filters lists every filter with what it catches in the open catalog and thread.
`enter` edits one (pattern, label, action, the fields it looks at, sites and boards, OPs
or replies only, and its options; a pattern that isn't a valid regex says why and isn't
saved), `space` turns it off or on
(`enabled = false`), `a` adds one and `x` removes one. Each change is written at once;
the rest of the config, comments included, stays as it was. A change that can't be written
holds only until ck quits, and the footer says so as an error. No message replaces an
error before it has been on screen (a second error joins it), and loading something new
clears one that has. In the config:

```toml
[[filter]]
pattern = "(?i)crypto|nft"     # a regex; (?i) for any case
label = "crypto"               # shown on what it hides or highlights

[[filter]]
pattern = "(?i)rust"
action = "highlight"           # "hide" (the default) or "highlight"
field = ["subject", "comment"] # see below; default subject + comment
sites = ["4chan"]              # optional: only these sites
boards = ["g"]                 #           and these boards
op = true                      # only OPs (threads); reply = true: only replies
enabled = false                # kept, but not applied
recursive = true               # replies to what it hides are hidden too (and theirs)
notify = true                  # a notification when a watched thread brings one
top = true                     # what it highlights comes first in catalogs

[[filter]]
pattern = "u8Vh17KxaDvUJ6bBcmE/eg=="   # field = "md5": a file's MD5 (base64, as 4chan shows it)
field = "md5"

[[filter]]
pattern = ">4MB"               # field = "filesize": a range, not a regex
field = "filesize"
```

The fields:

| field | what the pattern is matched against |
|-------|-------------------------------------|
| `subject`, `comment` | the subject; the comment's text, a line per line, spoilers included |
| `name` | the name as shown, with the tripcode and capcode |
| `filename`, `md5` | each file's name; each file's MD5 in base64, compared exactly (not a regex) |
| `id` | the poster ID, on boards that have them |
| `flag` | the country or board flag: its code (`US`) or its name (`United States`) |
| `tripcode`, `capcode` | the tripcode alone (`!!Fz3mQwerty`); the capcode (`mod`, `Admin`, …) |
| `dimensions` | each file's width and height, as `1920x1080` |
| `filesize` | each file's size against a range, not a regex: `>2MB`, `>=2MB`, `<100KB`, `<=100KB`, `1MB-5MB`, or one size; units `B`, `KB`, `MB`, `GB` (of 1024; any case), bytes without one |
| `postno` | the post's number, in digits (`(\d)\1$` for dubs) |

`notify = true` sends a desktop notification (the way `notify` says: see Watched threads) when
a watched thread refreshed in the background brings a new post the filter catches, or a
followed general's board has a new thread it catches, once each, together with the other
news of that round. (Not on ck's first look at a thread or board each session, which may
find old posts, nor for the thread you have open.) `top = true` on a highlighting filter
puts the threads it highlights at the top of the catalog, in the order the sort gives them
(before watched threads, with `watched_first`).
`op = true` and `reply = true` together catch nothing, and are refused.

### Hidden words

`hidden_words = ["crypto", "free money"]` hides every post and thread with one of the
words, on every site and board: plain words (no regex), in any case, as whole words
(`cat` hides "Cat pics", not "concatenate"; `c++` and `:^)` work as typed), with any spaces
between the words of a phrase. They're looked for in the subject, the comment (spoilers
too), the name and file names. What they hide is hidden like anything else: collapsed to a
line, shown by `Z` marked "hidden", counted in the catalog's header. Settings ›
Filters › Hidden words lists them (`a` adds one, `x` removes one), and `w` in a post's `X`
adds a word from it (the thread's search to start with). Either way it's saved at once, and
`u` right after takes it back.

Highlighted threads and posts get the label as a chip and an accent stripe; the catalog's
header says how many are hidden. A bad pattern is reported at startup with its filter's
number.

## Watched threads and history

`w` watches the open thread, or the selected one in a catalog. The "Watched" entry at the
top of the Sites view lists watched threads from all sites with their post counts and how
many posts are new (posts you've hidden, or your filters and hidden words hide, aren't
counted, nor notified about); threads that 404 stay listed as "archived/deleted", with
nothing new (the totals on the Watched entry, its view and the title leave them out).
"History" lists the last 100 threads you opened. `x` removes an entry from either list.

The open thread refreshes in the background every 10 seconds and watched threads every
60 seconds (change with `refresh_thread_secs` / `refresh_watched_secs`; those are also the
minimums). A quiet thread is refreshed less often: each refresh that brings no new post
makes the next wait half as long again, up to ten times the interval (no more than 10
minutes, unless the interval itself is longer); new posts, opening the thread or `r` start
over. `refresh_backoff = false` (or Settings › Background refresh › Quiet threads) keeps
the intervals fixed.

On 4chan and vichan sites, Watched also shows which page of its board's index each thread
is on ("p7/10"), and the open thread's top bar says it too. A thread on the last page is
next to fall off, so it's marked "last page 10/10". That's one request per board
(`threads.json`), asked along with its watched threads' refreshes, at most once a
`refresh_watched_secs` round however many of its threads you watch, and forgotten once
none of them has been refreshed for a while (20 minutes, or two rounds if they're longer).
Other engines have no such list, and show nothing.

Posts that arrived since your last visit are marked "new", and a "new posts" line sits
between the last post you'd read and the first new one; `U` jumps to the first one.

A post the moderators delete while the thread is open stays where it was, marked
"deleted" (the top bar counts them: "1 deleted"); its quotes, replies, previews and
conversation still work, and it's never counted as new. That's kept in memory, for the
open thread: saved copies, exports and post counts are the thread as the site has it. An
answer with fewer than half the posts last known (shown, or counted for a watched thread)
is more likely broken than mass deletion: it's shown as it came, without keeping what it
left out, and the status line says so. It isn't a visit, doesn't change the watch list's
counts, and doesn't replace the saved copy (nor does `e`, `w` or `m` while it's shown).
The answer after it is judged against it, so a real mass deletion is taken one refresh
later. An answer with no posts, or with another thread than the one asked for (as a site
might answer a reply's number), is a failed load (`r` tries again); in the background it
leaves a watched thread as it was. FoolFuuka and LynxChan error answers that say "not
found" count as gone, like a 404 (on LynxChan, for boards and catalogs too); others show
their text.

### Opening what you've seen before

ck keeps the last copy of each catalog and thread you open in its cache directory
(`$XDG_CACHE_HOME/ck/pages`, at most `page_cache_mb`, 100 MB by default). Opening one again
shows that copy at once, marked "cached 3m ago" in the top bar, and the same request as
always refreshes it in place, keeping your place. It's never an extra request: the copy
also lets the first request after a restart ask whether anything changed, so an unchanged
thread costs a "not modified" reply instead of the whole thread. When the refresh fails, the
copy stays up, still marked (and "dead" if the thread is gone). Watched threads open from
their saved copy the same way. A start that restores your tabs fills them in at once, too.
The budget counts the pages only, not a page still being written; temp files left by a
crash (`*.tmp` and `*.part`, older than a day) are removed when the cache is trimmed, here
and in the thumbnail cache.

### Saved threads

Watched threads are saved as they refresh: each time new posts arrive, the thread's last
good copy is written to the data directory (`threads/<site>/<board>/<no>.json`, only when
something changed, and in the background so ck never waits on it). When a watched thread 404s, its copy is kept and marked dead, so the
thread isn't lost. Saving a thread as a page (in the `.` menu) saves a copy too, of any thread.

"Saved" on the home screen (or `:saved`) lists the copies, newest first, with "dead" on
threads that are gone and "watching" on ones you watch. `enter` opens one in the usual
thread view, read offline: the top bar says "saved 2h ago" (and "dead"), nothing in it is
refreshed or fetched, and everything else works (replies, search, the menu, the gallery).
Images come from the thread's download folder when you saved them, and
thumbnails from the thumbnail cache; the rest show as placeholders. On a copy of a thread
that's still up, `r` opens the live thread. `x` (twice) deletes a copy; unwatching a thread
keeps it.

`:saved WORDS` (or "search inside the saved threads…" in the Saved view's `.` menu) searches
every post of every copy, without the network, the way `/` searches inside a thread: the
words as typed, in any case, in a post's name, subject, file names and text (hidden
spoilers left out). Results come in as the copies are read, newest copy first, with where
each is and the words highlighted; `enter` opens the copy on that post, with the search set
so `n` / `N` go through it, and `esc` comes back. 500 MB of copies take about a second and a
half.

When the thread you open has 404'd and there's a copy, ck offers it ("a saved copy from
2h ago: enter opens it"), and a thread that dies while you read it becomes its copy.
Copies take at most `saved_max_mb` (500 MB by default; 0 for no limit): past that, the
oldest copies of threads you don't watch are removed, dead or not (one you saved as a page,
or stopped watching, counts too). A watched thread's copy is never removed, even when
watched threads alone take more.

### Following a general

Generals are threads that start over when they fill up (/lmg/, /hsg/, …). `F` on one (in
its thread, the catalog, or Watched) follows it: it's watched, and when it 404s or 4chan
says it hit its bump limit, ck looks for the next thread whose subject has the same
`/tag/` (or, without a tag, the same subject minus its number) in that board's catalog.
The newest match that isn't hidden (by hand, a filter or a hidden word, whatever `Z`
says) is watched and followed instead, and you're notified; a hidden one is passed over,
even for an older match. A thread that died
is dropped from Watched; one that's only full stays until it dies. The search is one
background catalog request, repeated at most every 10 minutes while nothing is found.
Watched shows "follows /lmg/" on followed threads; `F` again stops following.

### What's new in a catalog

Catalogs mark threads that weren't there on your previous visit with "new" (the header
counts them), and threads you've opened before show how many replies they've gained since,
like `+12`. A board's first catalog marks nothing new, even when you've opened some of
its threads before from elsewhere. This is kept per board in the data directory (threads
are forgotten a week after they leave the catalog, or, if you never saw them in it, a
week after you last opened them) and needs no extra requests.

Threads you watch are marked ◉. `watched_first = true` (or Settings › Watched first) puts
them first in every catalog, then the rest, each in the order the sort gives them. Threads
a `top = true` filter highlights still come before them: a filter is a rule you wrote to
come first whatever the sort, and watching is more of a sort of its own (a top thread you
watch comes first among the top ones).

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

The terminal's title says where you are and what's new, for when ck is in a tab or window
in the background: in a thread, the new posts below the screen ("ck: (3) /g/ Thread
subject"), elsewhere the watched threads' unread posts ("ck: (5) Watched"), with "(You)"
when some of them reply to your posts. The title from before is put back when ck quits
(terminals that can't do that are left with an empty one). `set_title = false` (or
Settings › Terminal title) leaves the title alone.

ck never posts, so it can't know which posts are yours: `m` marks the selected post as
yours (and watches the thread). Replies to it are counted in Watched ("1 reply to you"),
quotes of it read `>>123 (You)`, and they get their own notification. The marks show in
every tab at once; unwatching the thread (`w`, or `x` in Watched) forgets them.

ck keeps its state as JSON in `$XDG_DATA_HOME/ck` (default `~/.local/share/ck`):
`watched.json`, `history.json`, `recent_boards.json`, `board_prefs.json` (each board's
sort and layout), `hidden.json` (what you hid with `H`), `seen.json` (catalog threads
seen, for "new" and `+N`), `session.json` (your tabs, for the next start), and `saved.json`
with `threads/` (saved threads). Board
lists fetched from sites are saved there too (`boards/`), so a site's boards show up
instantly next time; they're refreshed quietly in the background once a day, and `r` in
the Boards view refreshes them now. Lists that come in several pages (LynxChan and
jschan board lists, FoolFuuka catalogs) show each page as it arrives.

These files, the config file, saved threads, exports and downloads land whole or not at
all: each write goes to a temp file of its own beside the file (`<name>.ck-<pid>-<n>.tmp`)
and is renamed into place, so two writes at once (or two ck's) never mix, and a failed
write removes its temp. A write to a symlinked state, config or saved-thread file goes to
the file it points to. Temps of that name that a crash or kill left (older than a day) are
removed the first time in a run that ck writes into that folder; a `<name>.tmp` left by an
older ck is not, and can be deleted by hand.

## Searching archives

`A` in a catalog searches the board's posts on a FoolFuuka archive: the site itself if
it's one (desuarchive, palanq, b4k), or the archive configured for it with `archive`
(4chan's boards use desuarchive in the default config). Results show each post with its
thread; `enter` opens the thread on the archive with the post selected, `esc` goes back.
Each page of 25 results is one request; going down past the last one (or `n`) loads the
next. Archives limit how often you can search; when they say no, ck shows their message.
Posts your filters or hidden words hide, or that you've hidden, are left out of the
results (the title says how many) until `Z` shows them, marked "hidden"; this goes for
`:saved` searches too, where replies to hidden posts are left out as in their thread
(`recursive_hiding`, or a `recursive` filter).

## Images

Catalog and thread views show thumbnails, and `v` opens a full-screen viewer. ck asks the
terminal which image protocol it supports (kitty, sixel, iTerm2) and falls back to unicode
half-blocks, which work everywhere. The detected protocol is shown in the `?` help's
title bar. Thumbnails are skipped in terminals narrower than 60 columns.

Animated GIFs play in the viewer, at up to 20 frames a second (frames are prepared in the
background, and scaled down if a long GIF would take too much memory). Videos open
externally with `i`. When the site or archive has only a file's thumbnail, the viewer
shows it and says so; `i` and `y` in the viewer, and `o` and `y` on the focused file in a
thread, act on the thumbnail's address and say so, and `d` says there's no file to save.
A spoilered file the archive didn't keep has no thumbnail either: the viewer, and opening,
copying or saving it, say neither is available. The menu offers only what there is.

Images load in the background through the same rate limiter as everything else, only for
what's on screen (or about to be), and are kept in a bounded in-memory cache. Thumbnails
are also cached on disk in `$XDG_CACHE_HOME/ck/thumbs` (default `~/.cache/ck/thumbs`, at
most 200 MB, least recently used first out), so revisiting a catalog or thread shows them
at once without any requests. On sites whose images share the page's rate limit, visible
thumbnails load top to bottom and nothing is prefetched ahead of them.

Inside tmux, the question (and the images) only get through with `set -g
allow-passthrough on`; without it ck doesn't ask, and uses half-blocks. Inside zellij, ck
uses half-blocks: zellij answers the question for the terminal it runs in, and says sixel
whatever that terminal is, so nothing would show in most of them. When
the detected protocol is wrong for your setup, name one: `images = "halfblocks"`,
`"sixel"`, `"kitty"` or `"iterm2"` (Settings cycles through them too; it takes effect at the
next start). To turn images off entirely (no image requests at all), put `images = "off"`
at the top of your config.

### Images on or off per board

"images on this board: on → off" in the `.` menu of a catalog, a thread or the Boards list
turns a board's images off (or back on; the action is `board_images`, without a key until
you give it one). With images off on a board, ck asks for none of its images: thumbnails
show "image off" in their place, the layout stays the same, `v` says images are off instead
of opening the viewer, and the top bar says "images off". Opening a file with `i`, links,
copying and downloads still work. On an overboard each thread follows its own board,
unless the overboard has a setting of its own.

`nsfw_images = "off"` turns images off on every board the site marks NSFW (4chan, jschan,
LynxChan and makaba say which; vichan sites don't, so their boards count as worksafe),
including boards you haven't opened yet. A board's own setting comes first, and
`images = "off"` still turns everything off. Settings › Catalog has both: "NSFW boards",
and "Board images", which lists the boards with their own setting (`x` takes one away).
Boards' settings are kept in the data directory with their sort and layout.

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

Only one key saves files: `d`, which saves the file in front of you (one you've focused
with `tab`, the one in the image viewer, or the one selected in the gallery). On a post
with nothing focused it says how instead of saving anything. The `.` menu saves more: all
of a post's files, all of the thread's ("save all the thread's files…": the whole
thread's, even in a conversation, but not hidden posts' unless `Z` shows them), or the
thread as a page ("save the thread as a page…"). The last two first say what they'll
write (how many files, about how big) and where; `enter` saves, anything else cancels. To
have keys for these, give `download_post`, `download_thread` or `export` one (see
[Remapping keys](#remapping-keys)); they still ask first.

Files go into `~/Downloads/ck/{site}/{board}/{thread}/` (your system's Downloads folder).
Files are named `{post}_{original name}` with unsafe characters replaced; files that
already exist are skipped, so saving all the thread's files again later only fetches
what's new. Progress shows at the right of the footer. Downloads go through the same rate
limiter as images. A file lands whole or not at all, like ck's state files: a download that
fails leaves nothing behind, and a download never writes through a symlink at its name.
When the site or archive has only a file's thumbnail, saving skips it.

### Saving a thread

"Save the thread as a page…" in the `.` menu writes the open thread into its download
folder as `thread.html`, a page with the current theme's colors that reads offline, and
`thread.json`, the posts as data (`"format": 1`). Files already saved there are shown from
the folder; the others link to the site (to the thumbnail when that's all the site has,
and nowhere when it has neither). So saving all the thread's files and then the page
makes a complete offline copy. Saving again replaces both. Each file in `thread.json` has
a `kind` (`image`, `video` or `other`); a file the site or archive has only the thumbnail
of has `"url": null`. The thread is also saved for ck itself, to open from the Saved view;
there such a file's `url` is `""`, so older versions of ck still read the copy.

## Themes and settings

ck draws flat: no lines or boxes, just areas of color. The screen is the darkest tone,
catalog entries and posts are cards a step lighter, and popups another step up; the
selected row, card or post is tinted and marked by a stripe in the theme's accent color.

`,` opens Settings. They open over where you are: what's loading there goes on loading
and lands under them (a post looked up opens its thread there), and `esc` closes them on
wherever that left you. The thread under them isn't refreshed as the open one meanwhile:
if it's watched, its new posts count as unread, as any watched thread's do. `:` from
Settings closes them and goes from the place under them. Their options are saved to your
config file, keeping your comments (a symlinked config is written through the link, and
keeps its permissions), and the file is created from the default if you don't have one
yet:

- **Theme**: pick from the built-in themes (material, material-light, nord, gruvbox,
  catppuccin, tokyo-night, solarized-light, terminal, which uses your terminal's own
  colors on its own background, light or dark (cards, panels and bars aren't filled, and
  the selection shows by its bar), and mono, which uses none: the default when `NO_COLOR`
  is set) and your own.
  Nothing depends on color alone: the selection has a bar, a focused part is bold and
  underlined, and spoilers are shaded out until revealed. The screen changes as you move through the list; `enter` keeps
  the theme, `esc` goes back.
- **Colors**: every color the theme uses, with a swatch and what it's for. `enter` edits
  one (`#rrggbb`, a name, or a 256-color index), `x` resets it. Changing a built-in theme
  saves your changes as a copy, `[themes.NAME-custom]`.
- Color depth, the terminal title, the catalog layout, images, filters, hidden words and replies, the refresh
  intervals, the download folder, and key bindings (see [Remapping keys](#remapping-keys)).

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
- `refresh_thread_secs`, `refresh_watched_secs`, `refresh_backoff = false`
- `watched_first = true` puts watched threads first in catalogs (see
  [What's new in a catalog](#whats-new-in-a-catalog)).
- `catalog_layout = "cards" | "compact" | "grid"`: the default layout (also in Settings).
  `c` in a catalog and `s` set that board's own layout and sort, which are remembered in
  the data directory (`board_prefs.json`). In the grid, `h`/`l` move between columns
  (`h` in the first column goes back) and `j`/`k` between rows; without images it's
  shown as cards.
- `download_dir = "~/stuff/{site}/{board}/{thread}"`
- `restore_session = false` to start at the site list instead of where you left off (the
  view, thread and selected post, catalog sort and filter, saved in the data directory
  as `session.json`; a thread or catalog that failed to load is kept as it was asked
  for, and `r` tries it again the same way). `ck URL` always starts at the URL.
- `favorites = ["4chan/g", ...]`, `hidden_sites = ["wizchan", ...]`: see
  [The home screen](#the-home-screen).
- `notify = "auto" | "bell" | "off"`, `notify_command = [...]`: see
  [Notifications](#notifications).
- `set_title = false` to leave the terminal's title alone (see
  [Notifications](#notifications)).
- `[[filter]]`: hide or highlight threads and posts, see [Filters](#filters-and-hiding).
  `recursive_hiding = true` hides replies to hidden posts too.
- `[[image_search]]`: reverse image search engines, see
  [Reverse image search](#reverse-image-search).
- `[[site]]`, `default_sites = false`: see [Adding sites](#adding-sites).
- `archive = "desuarchive"` on a `[[site]]`: the archive for 404'd threads and `A` searches.
- `[keys]`: `action = "key"`, `action = ["key", ...]`, or `action = []` for none (the `.`
  menu only), e.g. `watch = "W"`; see
  [Remapping keys](#remapping-keys). Unknown actions, things that aren't keys, and two
  commands on one key in the same view are reported at startup.
- `theme = "nord"`, `[themes.NAME]`, `color = "auto" | "truecolor" | "256"`: see
  [Themes and settings](#themes-and-settings). A ck 0.2 `[theme]` table of colors still
  works, on top of the default theme.

## Adding sites

Paste a link to any page of the site after `:` (or in Settings › Sites › Add a site, or
"add a site…" in the home screen's `.` menu). ck asks the site's APIs what it runs (jschan,
LynxChan, FoolFuuka, vichan or makaba, one request a second like any other), and shows
what it found with a name for it, which you can change: it's what you type after `:`
(`somechan/b`) and what favorites are saved under. `enter` adds it to the config as a
`[[site]]`, and from `:` goes on to the link.

vichan sites have no board list API, so ck reads the boards from the bar at the top of the
site's pages (the board's page for a link to a board, the front page for a link to the
site): links to `/x/` with a title, the way vichan writes its boards. Pages like the rules,
and overboards, are left out. If a site's pages have no such bar, it starts with the
board of the link you pasted. Pasting a link to another of its boards in Settings › Sites
› Add a site adds that board (once ck has checked its catalog is there).

ck also works out where a vichan site's files are, from the thread previews on the same
page: thumbnails all in one format whatever the file's (vichan's `thumb_ext`), and files
on another host (`media_url`, like 8kun's). When the page has none to go by (a site whose
pages are drawn in the browser), it asks for one thumbnail from the board's catalog in a
couple of formats (at most three small requests). The popup says what it found.

Settings › Sites › Your sites lists the `[[site]]` tables in your config; `x` twice takes
one out, and `r` on a vichan site reads its board bar again (so does "update the board
list…" in its Boards list's `.` menu). It shows what changed (new boards, new titles, and
boards no longer in the bar, which are kept unless `d` drops them; some boards are left
out of the bar on purpose) and `enter` writes the new list. A built-in site updated this
way becomes one of your sites, with that list.

The built-in sites are always there, whatever your config holds, so new ones (and fixes to
them) come with new versions of ck. A `[[site]]` with a built-in site's name replaces it
(`ck --print-sites` prints them all, to copy one and change it), and
`default_sites = false` leaves out every built-in site. By hand, a site looks like this:

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

Config files from earlier versions of ck hold a copy of every built-in site (ck used to read
only the sites in the file). Those copies still work (each replaces the built-in site of its name); delete
the ones you never changed to get the built-in versions.

## Tests

    cargo test                                        # offline: unit, parsing, snapshot and short fuzz tests
    cargo test -- --ignored live --nocapture          # hit every default site live (rate-limited)

The offline tests parse real, trimmed responses from every engine in `tests/fixtures/`, and
render every view with fixed data and a fixed clock into the snapshots in `src/snapshots/`
([insta](https://insta.rs)). After an intended UI change, review the differences and update
them with `INSTA_UPDATE=always cargo test` (or `cargo insta review`).

### Fuzzing

Nothing here leaves the machine: fake sites answer on `*.invalid` hosts or on 127.0.0.1.

    cargo test --profile fuzz -- --ignored _long --nocapture   # every fuzzer, for longer
    cargo build --release && cargo test -- --ignored e2e_soak --nocapture
    cd fuzz && cargo fuzz run -O backend_json corpus/backend_json ../tests/fixtures

- **The app** (`src/app/fuzz.rs`): random keys, clicks, pastes, resizes, time passing and
  restarts, against fake sites (generated data, or the real backends on mangled fixtures)
  whose answers arrive in any order. After every step it draws a frame and checks the
  tabs, threads, layouts and popups; nothing may stay loading, no worker thread may panic,
  nothing may be fetched in the background twice within 10s, and the saved data must load
  again. Threads die for good now and then: a watched thread that was ever loaded must
  keep its saved copy, and a dead thread's copy being read is never fetched. A filter
  added from a post must catch it, and after any filter change the marks and the config
  agree with the filters in use. A conversation shows exactly its posts, and `M` the posts
  with files. A post shown before a refresh of the same thread is still there after it
  (or marked deleted), unless the refresh came back with fewer than half the posts last
  known, and a deleted post is never new. The fake sites now and then answer with no posts
  or with another thread, as a site in trouble might. A post hidden as a reply quotes a
  hidden post. A failure prints the seed that replays it and the fewest steps that still
  fail (`FUZZ_TRACE=1` prints the state after each step).
- **The rest** (`src/fuzz.rs`): every engine's parsers on mangled responses, the markup
  parser and wrapping, routes, the rate limiter and the cache against a model, broken data
  directories and saved threads (nothing the user had may be lost), broken configs and
  filter edits through the config writer, and broken images.
- **Coverage-guided** (`fuzz/`, [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz),
  nightly): targets for markup, routes, API JSON, the config, data files and images.
- **End to end** (`src/e2e.rs`): the release binary in tmux, against local servers that
  answer like the engines, slowly or badly, with random keys, text, resizes and restarts
  for `E2E_SECS`. It must stay up, quit cleanly, keep its memory flat and leave data that
  loads, and what's on screen must be exactly what ck drew. `CK_NO_EXTERNAL=1` keeps ck
  from opening a browser or player, writing the clipboard or notifying; `CK_FRAME_DUMP=file`
  writes each frame as ck means it to look (to compare with the screen when something's off),
  and `CK_INPUT_LOG=file` writes a timed line for every input event as it's read and as it's
  handled, and for every frame drawn (to see what happened to a key). The soak also checks
  that the first key after starting is acted on.

`FUZZ_SEED`, `FUZZ_RUNS` or `FUZZ_SECS`, and `FUZZ_STEPS` steer the runs. CI runs the tests
on every push; the fuzzers run nightly.
