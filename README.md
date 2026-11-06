<p align="center">
  <img src="docs/assets/logo.svg" width="96" height="96" alt="Iris">
</p>

<h1 align="center">Iris</h1>

<p align="center">
  A fast, quiet mail client — with your calendar, tasks and notes beside it.<br>
  Built for getting through your mail, not for filing it.
</p>

<p align="center">
  <a href="https://github.com/Alicienn/iris/releases/latest"><img src="https://img.shields.io/github/v/release/Alicienn/iris?label=version&color=555" alt="Version"></a>
  <img src="https://img.shields.io/badge/Windows-10%20%7C%2011-0078d4" alt="Windows 10 and 11">
  <img src="https://img.shields.io/badge/Android-8%2B%20(preview)-3ddc84" alt="Android 8 and later, preview">
  <a href="https://github.com/Alicienn/iris/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/Alicienn/iris/ci.yml?branch=main&label=CI" alt="CI"></a>
  <img src="https://img.shields.io/badge/license-MIT%20%7C%20Apache--2.0-555" alt="License">
</p>

---

## Highlights

- <img src="docs/assets/icons/inbox.svg" width="16" alt=""> **A work queue** — every conversation is *to do*, *waiting* or *done*; one key moves it along.
- <img src="docs/assets/icons/users.svg" width="16" alt=""> **All your accounts** — one unified inbox, or one mailbox at a time, grouped by tag.
- <img src="docs/assets/icons/sun.svg" width="16" alt=""> **Home** — your day in one sentence and the next things to do.
- <img src="docs/assets/icons/calendar.svg" width="16" alt=""> **Calendar** — month, week, day, 3 days and agenda; CalDAV both ways; invitations from mail.
- <img src="docs/assets/icons/check-circle.svg" width="16" alt=""> **Tasks** — typed in one line (`tomorrow 9am Call Marie #Work !!`), planned into your free time.
- <img src="docs/assets/icons/target.svg" width="16" alt=""> **Goals and habits** — the pace a goal needs, habits ticked day by day with their streaks.
- <img src="docs/assets/icons/file-text.svg" width="16" alt=""> **Notes** — Markdown files written as they look, with LaTeX, flashcards and spreadsheets.
- <img src="docs/assets/icons/refresh.svg" width="16" alt=""> **Sync between your devices** — over your Wi-Fi, paired with a QR code, no account and no cloud.
- <img src="docs/assets/icons/smartphone.svg" width="16" alt=""> **Any width** — the same Iris on a wide screen, a narrow window, a foldable or a phone.
- <img src="docs/assets/icons/zap.svg" width="16" alt=""> **Instant** — opening, archiving and searching never wait, even with hundreds of thousands of messages.
- <img src="docs/assets/icons/shield.svg" width="16" alt=""> **Private** — remote images and trackers blocked by default; no telemetry.
- <img src="docs/assets/icons/layers.svg" width="16" alt=""> **Sorting** — rules, VIP, office hours and filing, on mail as it arrives.

## Install

| Platform | How |
|---|---|
| **Windows 10 / 11** | Download `iris-setup-<version>.exe` from [**Releases**](https://github.com/Alicienn/iris/releases/latest) and run it. No administrator rights needed. |
| **Android 8+** (preview) | Download `iris-<version>.apk` from the same release and open it on the phone (Android asks to allow installs from the app that opened it). |

- The installer is not signed yet: if Windows shows *Windows protected your PC*, click
  *More info*, then *Run anyway*.
- Iris updates itself: an **Update now** button appears in the status bar when a new
  version is out. Every update is signed and checked before it runs.
- To uninstall on Windows: *Settings › Apps*. Your mail and settings are kept.

## Getting started

1. Open Iris: with no mailbox yet, it starts on a welcome page.
2. **Add your first account**: your address and password. Iris finds the servers on
   its own and tells you where it found them.
3. Your mail arrives, inbox first.

Already using Iris on another computer? Choose **Set up from another device** instead
(see [Sync between devices](#sync-between-devices)).

More ways to add an account:

- **Import a profile** — a `.mobileconfig` from your host or IT department fills in
  the servers (signed profiles too).
- **Configure manually** — servers, ports and a user name when the login is not the
  address. Nothing is ever sent unencrypted.
- **Google or Microsoft** — set your own OAuth client in *Settings › Sign in with
  Google or Microsoft*, then sign in through the browser. Without one, use an
  [app password](https://myaccount.google.com/apppasswords).

Passwords are kept in the Windows credential store, or in an encrypted vault whose key
the Android Keystore holds.

## Getting around

- The five places — **Home**, **Mail**, **Calendar**, **Tasks**, **Notes** — sit at
  the top of the side column (`Ctrl`+`0` to `Ctrl`+`4`), in a rail on a medium window,
  and at the foot of the screen on a phone.
- **Back** and **Forward** (`Alt`+`←` / `Alt`+`→`, or the mouse's side buttons) return
  to the places you visited, as in a browser.
- `Ctrl`+`K` opens every command, narrowed as you type; `?` lists the keys; pointing at
  a button shows its key.
- *Settings* (`Ctrl`+`,`): appearance (light, dark or as the system is), accent colour,
  density, text size, **Reduce effects** (nothing slides, fades or springs; Iris also
  follows the system's own animation setting), window buttons.

### On a phone or a narrow window

- Below 600 pixels each place shows one thing at a time: the list, then the
  conversation, task, habit or note as a page of its own.
- Mail opens on a large title (*All accounts* ▾, which opens the mailboxes and
  folders), how much is to do, and the queues across the screen; **New message**
  floats over the list. *Lists* opens the tasks' lists.
- Questions rise from the bottom as sheets; forms fill the screen.
- *Settings* is a list of groups, each opening as a page.
- Swipe a conversation right for **Done**, left for **Snooze**, with **Undo** for a few
  seconds; a long press opens a row's menu. Pull the list down from its top to check
  for new mail.
- Under a conversation: **Delete**, **Archive**, **Snooze**, **Done** and **Reply**; a
  tap on Reply offers Reply all and Forward, a second tap replies.
- Ticking a task or a habit, a swipe, a choice or a delete gives a light vibration,
  finishing the day's habits or reaching inbox zero a firmer one (the phone's own
  touch-feedback setting turns them off).
- A note being written has formatting keys under it. Hold a note or a folder a moment
  to drag it into a folder; held over a closed folder, it opens.
- New message's formatting keys format the words selected, or what is typed next.
- At the end of the setup a phone asks for notifications and to run in the
  background; *Settings › Notifications* warns while Android holds either back.
- Android's home screen can show the **Iris · Today** widget: what is next, the tasks
  due and the mail to answer. A tap opens Iris.
- *Settings › Updates › Copy the energy report* copies what Iris used minute by minute
  (CPU by thread, memory), to paste in a report when the phone warms up.
- `Escape`, or Android's Back, closes what is open, then goes back.
- On a phone, Home's magnifier searches mail, tasks, events and notes at once.
- Tapping the tab you are in returns to its first screen (Mail's list, today in the
  calendar).
- If Iris stopped unexpectedly, the next start says what and where, to report it.

## Mail

- **Queues**: **To do**, **Waiting**, **Done** in the toolbar. *To do* empty is
  **inbox zero**, and Iris counts your streak.
- **Acting on a conversation**: **Done** `E`, **Snooze** `S` (later today, tomorrow,
  this weekend, next week), **Waiting** `W`, **To task** `T`, **Archive** `A`, star `F`,
  delete. Point at a row to do it without opening it, or right-click it (**Mark as** ›
  *To do*, *Waiting*, *Done*); `Ctrl`+`Z` undoes, on the server too.
- **Reading**: a conversation you open is marked read, on the server too; `R` marks
  it unread, or read again. Double-click a word to select it, triple-click for its
  paragraph; a selection can start in the margin.
- **States follow you**: *Waiting* and *Done* are written on the server, so Iris on
  your other computers finds them.
- **Writing**: address chips, attachments, formatting, signatures, aliases (*Send
  as…*), **Send later**, and an **Undo** of a few seconds after sending (*Settings ›
  Undo send*), from the notice or with `Ctrl`+`Z` wherever you are. Send asks first
  when your words mention an attachment ("attached", "ci-joint"…) and none is attached;
  in a reply only what you wrote counts, never the earlier messages.
- **People in a message**: right-click the sender or a recipient to copy their address
  or name, write to them, or see their mail.
- **Words to a task**: select words in a message, right-click, **Make it a task**; the
  task links back to the message.
- **Opening Mail** shows *All accounts* and *All folders*; the Inbox is a click away.
- **Replying**: `Ctrl`+`R`, or `Ctrl`+`Shift`+`R` for everyone; the earlier messages sit
  in the writing field under what you write, each folding on its own. Click a recipient
  for their card, with buttons to copy the name and the address.
- **Trackers**: a message that tries to learn when you read it says so; **Ignore** keeps
  its remote content blocked, **Allow** loads it, and the warning goes either way.
- **Searching** (`/`): free text, each word found as you type it (`réu` finds
  *réunion*), and `from:`, `is:unread`, `has:attachment`, `newer_than:7d`, or the pills
  under the field.
- **Folders** are one name across your mailboxes, made on all of them or the ones you
  tick.
- **Tags** group your mailboxes (*Clients*, *Personal*…); click one to see its mail
  only.
- **All at once**: right-click a mailbox, a tag's title or a folder for **Mark all as
  read**; a mailbox or a tag also has **Send all to** *To do*, *Waiting* or *Done*.
  It takes thousands of messages in one go, and never brings back what is already
  *Done*.
- **Invitations** show as a banner: **Accept**, **Maybe** or **Decline**, and the
  calendar follows.
- **Unsubscribe** beside a mailing list's sender, in one click.
- **Sorting** (*Settings › Sorting*), on new mail, each off until you fill it in:
  **VIP** pins mail from the addresses or `@domains` you list and puts it back to do;
  **Office hours** holds mail that arrives outside your working hours (in UTC, weekends
  optionally) until the next working morning; **Filer** files a conversation into the
  first folder whose `pattern -> Folder` line matches its subject or an attachment's
  name (the folder must exist). **Rules** (*Settings › Rules*) mark mail from a sender
  or a domain done as it arrives, and can be tried on past mail first.
- **Safety**: links open only in your browser; attachments are marked as downloaded,
  so Windows warns before running them.

New mail is checked at least every quarter of an hour, every two minutes for the
mailbox on screen, at once when the server announces it, and with `F5`. On Windows the
taskbar button shows how many messages are unread, and clicking a notification opens
the message, event, task or habit it is about, even when Iris was closed.

## Calendar

- Views: **Month**, **Week**, **Day**, **3 days** and **Agenda** (`M` / `W` / `D`;
  `T` today, the arrows move).
- Click a free slot and type a name to add an event; drag it to move it, drag its edge
  to stretch it. `Ctrl`+`Z` undoes; adding, editing and deleting an event also say so
  in a message with an **Undo** button. Calendars are coloured boxes to tick; today and
  your selection follow the accent colour.
- Video calls: **Join on Meet** (or Teams, Zoom, Webex) on the event and on Home.
- Your own calendars, calendars **subscribed by link** (`webcal://`, `.ics`), and
  **CalDAV accounts** (iCloud, Fastmail, Nextcloud, Google…) kept both ways.
- **Import an `.ics` file** from the Calendar's toolbar into your own calendar.
- A subscribed calendar's events can be deleted, moved or renamed; that holds until the
  calendar itself changes them, on the device where you did it.
- Drag today's tasks onto the week to book time for them; open an event to add notes
  and tasks to it.

## Tasks, goals and habits

- **Today**, **Upcoming**, **All tasks**, **From mail**, and your own lists.
- Type a task in one line: date, time, list and priority are read from it
  (`vendredi 14h30 Dentiste`, `in 2 weeks Renew the passport #Admin !!!`).
- Subtasks, notes, reminders, how long it takes, repeats (every day, weekday, week,
  month, year).
- **Plan my day** lays the day's tasks into your free time; **Find a slot** books one;
  **Later** (`L`) puts one off.
- **Goals**: a number or milestones by a date, the pace needed, time blocked weekly in
  the calendar (*Make time for it*).
- **Habits**: every day, some days or so many times a week; streaks, reminders, an
  amount with **−** and **+**. Type one in a line: `Run 3 times a week`, `No phone in bed`.
- **Backups**: your calendars, tasks, goals and habits are copied every day (fourteen
  days kept); *Settings › Your calendars and tasks* restores one or exports `.ics`.

Keys: `N` new, `J` / `K` move, `Space` done, `D` due date, `L` later, `Delete`.
Right-click a task to delete it or mark it done. **Clear** puts the completed tasks in
an archive: they leave the list but still count in the statistics.

## Notes

Notes are Markdown files in folders on your disk (a **space**), so any app can read
them; an Obsidian vault opens as it is.

- **Written as they look**, in Inter: headings, lists, checkboxes and quotes are
  converted as you type; only the mark under the cursor shows its signs, and a
  heading's `#` show, dimmed, when the cursor is at its start.
- **Tables** are written as a grid: `Tab` goes to the next cell, `Enter` adds a row
  (on an empty last row, it leaves the table), the `+` on its edges add a row or a
  column.
- **Pictures** keep their proportions; drag their left or right edge to resize them.
- **Right-click** selected words to format, copy or make them a task, or a picture to
  copy it, open it or put it back to its size.
- **Maths** in LaTeX, drawn as in a book, with `Tab` shortcuts (`//` a fraction, `sq` a
  root, `mat2`…).
- **Links** between notes (`[[Note]]`, `@Note`), renamed everywhere when a note moves;
  a **graph** of them (`Ctrl`+`G`).
- **Spreadsheets** (`.sheet`) with formulas, formats and Excel/CSV export, shown in a
  note with `![[Budget.sheet]]`.
- **Charts** of a spreadsheet's table (the toolbar's chart button, from the cells
  selected): a flow from a whole to its parts, bars, columns, a donut or a line, each
  in a tab beside the sheets. They count the rows by a column of categories, or add up
  a column of numbers, and take the colour of their cells' text. A flow goes through
  several columns in turn; a category written `Sent > Interview` is two steps; two
  columns naming each other (`Income → Net`, `Net → Rent`) chain their bands. **Copy
  for a note** gives `![[Budget.sheet#Spending]]`, shown in any note and following the
  sheet; `/chart` offers them too.
- **Flashcards** (`Question :: Answer`), revised with spaced repetition.
- **Tabs**, each back where you left it, a note **beside** another (`Ctrl`+`\`),
  reading mode (`Ctrl`+`R`), focus mode (`F11`), version history.
- `Ctrl`+`←` / `Ctrl`+`→` go to the line's start or end; `→` right after typing a mark
  steps out of it (`**word**|`).
- **With the rest of Iris**: links to a conversation, an event or a task; a checkbox
  made a task stays ticked with it.

| Typed | Gives | Key |
|---|---|---|
| `**x**`, `*x*` | Bold, italic | `Ctrl`+`B`, `Ctrl`+`I` |
| `__x__`, `--x--` | Underline, strikethrough | `Ctrl`+`U`, `Ctrl`+`Shift`+`X` |
| `==x==` | Highlight | `Ctrl`+`Shift`+`H` |
| `{r}x{/}` | Coloured text | `Ctrl`+`Shift`+`1`…`7` |
| `` `x` `` | Code | `Ctrl`+`E` |
| `$x$`, `$$` | A formula in the line, on its own | `Ctrl`+`M` |
| `[[Note]]`, `@Note` | A link to a note | `Ctrl`+`L` |
| `[text](https://…)` | A web link | `Ctrl`+`K` |
| `# ` … `###### ` | Headings | `Ctrl`+`H` |
| `- `, `1. `, `[] ` | Bullets, numbers, a checkbox | `Ctrl`+`Enter` ticks, `Tab` nests (numbers start at 1) |
| `\| a \| b \|` | A table | `Tab` next cell, `Enter` new row |
| `>def `, `>thm `, `>tip `… | A callout, numbered | |
| `Question :: Answer` | A flashcard | |
| `/` | Every block, with a preview | |

## An AI assistant

An assistant that speaks MCP (Claude Desktop, Claude Code and others) can work in Iris
for you: search the mail with every filter, read conversations, mark, file, snooze and
send; see the calendar by day or week, create, move and delete events with their notes,
meeting links and subtasks; manage tasks, habits and goals; browse note spaces, read and
write notes in Iris's full syntax, and fill spreadsheets and charts.

1. *Settings › System › **Let an AI assistant use Iris***.
2. **Copy setup**, and paste it into the assistant's MCP settings
   (`claude_desktop_config.json` for Claude Desktop). For Claude Code:
   `claude mcp add iris -- "<path to iris.exe>" mcp`.
3. Restart the assistant. It starts `iris mcp`, which reaches the Iris that runs (and
   starts it in the notification area if it does not).

What the assistant changes shows in Iris at once and syncs like your own clicks; a mail
it sends waits in the outbox for the undo delay. Turning the setting off cuts it off at
once. Desktop only: there is no assistant access on Android.

## Sync between devices

1. On one device: *Settings › Devices › **Pair a device…*** shows a QR code.
2. On the other, on the same Wi-Fi: on a phone, *Scan a code* (or *Set up from another
   device* on the welcome page), or scan it with the phone's own camera, which opens
   Iris on it; on a computer, paste the code (**Copy the code**, then *Join with a code*).
3. Both show the same six digits, and the first sync runs; the joining device ticks off
   accounts, tasks, calendars, settings and notes as they arrive. Scanning on Android
   needs Google Play services and its camera permission; when it is refused, Iris says
   where to allow it, or the code can be typed (*Join with a code*).

What travels: task lists and tasks, goals, habits, your own calendars and events, your
accounts with their passwords, tags, rules, shared settings (VIP, office hours and
Filer among them) and note spaces. Mail itself comes from your servers on each device;
what you sort it into — To do, Waiting, Done, a snooze — travels, even to mail the other
device has not fetched yet. Newer mail in the conversation on the other device wins
over a choice made before it arrived.

- Paired devices sync on their own when they meet on the Wi-Fi, or with **Sync now**.
  A device that left the Wi-Fi shows a red dot, and **Sync now** says so.
- **Remove** beside a device forgets it and its key; pair it again to sync with it.
- A change made on both sides keeps the newer one, and Iris names it.
- A note changed on both keeps the other version in its history; a deleted note goes to
  the Recycle Bin.
- An account removed on one device is never removed on the other without asking.
- The code works once, for five minutes.

## Keyboard shortcuts

| Key | Action |
|---|---|
| `E` / `A` / `S` / `W` | Done / Archive / Snooze / Waiting |
| `R` / `F` / `Shift`+`3` | Read or unread / Star / Delete |
| `T` | Make the conversation a task |
| `C` | New message |
| `Ctrl`+`R` / `Ctrl`+`Shift`+`R` | Reply / Reply all |
| `/` or `Ctrl`+`F` | Search the mail |
| `F5` | Sync all accounts |
| `Ctrl`+`0` … `Ctrl`+`4` | Home / Mail / Calendar / Tasks / Notes |
| `Alt`+`←` / `Alt`+`→` | Back / Forward |
| `Ctrl`+`K` | Commands |
| `Ctrl`+`,` | Settings |
| `Escape` | Close, leave full screen, a search or a selection |
| `Ctrl`+`N` / `Ctrl`+`O` / `Ctrl`+`Shift`+`F` | Notes: new / open / search |
| `Ctrl`+`T` / `Ctrl`+`W` / `Ctrl`+`Tab` | Notes: new tab / close / next tab |
| `Ctrl`+`←` / `Ctrl`+`→` | Notes: the line's start / end |
| `?` | Every key |

## Current limits

- **Android is a preview**: mail is fetched only while Iris runs (behind the screen
  too once allowed to); opening files and Google or Microsoft sign-in are not wired
  (attaching a file uses the phone's picker).
- Iris speaks IMAP; POP accounts are not supported. Iris ships without an OAuth client
  of its own for Google and Microsoft.
- Attachments are limited to 25 MB per message, as on most servers.
- Replies and forwards are introduced in English ("On …, … wrote:").
- Outlook and Microsoft 365 calendars can only be subscribed to by link, read-only.
- Sync between devices needs both on the same Wi-Fi; a note space added from elsewhere
  does not travel, nor does a task's link to its conversation.
- Notes: input methods (Chinese, Japanese, Korean) do not type yet; spreadsheets have no
  merged cells or conditional formats; a chart reads one table of one sheet, shows at
  most 12 shares or 10 steps per column (the rest as "Other"), and is not exported to
  Excel.
- Office hours are set in UTC. Filer reads an attachment's name only once the message's
  body has been fetched. Iris has no plugins since 7.0.
- The command line only starts, checks (`iris doctor`), registers Iris
  (`iris register`, `iris unregister`) and serves an assistant (`iris mcp`); accounts
  are added and synced in the window.
- An assistant reaches Iris only while Iris runs on the same computer; it cannot add
  accounts, change settings or answer invitations.

## Privacy

- Iris talks only to your mail and calendar servers, to the calendars you subscribe to,
  to a list's address when you unsubscribe, and to GitHub for updates (a request that
  says nothing about you).
- *Waiting* and *Done* are written on your mail server as the keywords `$IrisWaiting`
  and `$IrisDone`; other apps can see them, and they say nothing else.
- Once you pair a device, Iris announces itself on your local network by a random
  device number, nothing else. What paired devices exchange is encrypted with a key
  only they hold, and never leaves the Wi-Fi.
- Backups and notes stay on your devices. Removing an account removes its mail, its
  passwords and its search index from the device.
- With *Let an AI assistant use Iris* on (off by default), Iris listens on your
  computer only (127.0.0.1) and answers only a program that holds the key it writes in
  your profile. What the assistant reads is then sent to its AI service: turn it on only
  for one you trust.
- On Windows Iris registers the `iris-mail:` link type for your user (so a notification
  can open what it is about); uninstalling removes it.

## Reporting a problem

Open an [issue](https://github.com/Alicienn/iris/issues) saying what you were doing and
what happened. Never include a password or the content of a private message.

---

<sub>To contribute or see how Iris is built, read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
Licensed under MIT or Apache-2.0, at your option; the Inter font, under the SIL Open Font License.</sub>
