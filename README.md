<p align="center">
  <img src="docs/assets/logo.svg" width="96" height="96" alt="Iris">
</p>

<h1 align="center">Iris</h1>

<p align="center">
  A fast, quiet desktop mail client.<br>
  Built for getting through your mail, not for filing it.
</p>

<p align="center">
  <a href="https://github.com/Alicienn/iris/releases/latest"><img src="https://img.shields.io/github/v/release/Alicienn/iris?label=version&color=555" alt="Version"></a>
  <img src="https://img.shields.io/badge/Windows-10%20%7C%2011-555" alt="Windows 10 and 11">
  <img src="https://img.shields.io/badge/license-MIT%20%7C%20Apache--2.0-555" alt="License">
</p>

---

## Highlights

| | |
|---|---|
| <img src="docs/assets/icons/sun.svg" width="20" alt=""> | **Home.** Your day in one sentence and the next three things to do, one click from your mail, tasks and calendar. |
| <img src="docs/assets/icons/inbox.svg" width="20" alt=""> | **A work queue.** Every conversation is *to do*, *waiting* or *done*. One keystroke moves it along. |
| <img src="docs/assets/icons/users.svg" width="20" alt=""> | **All your accounts in one place.** A unified inbox, or one account at a time, without slowing down. |
| <img src="docs/assets/icons/zap.svg" width="20" alt=""> | **Instant.** Opening, archiving and searching never make you wait, even with hundreds of thousands of messages. |
| <img src="docs/assets/icons/shield.svg" width="20" alt=""> | **Private.** Remote images and tracking pixels are blocked by default. No telemetry. |
| <img src="docs/assets/icons/layers.svg" width="20" alt=""> | **Modules.** Rules and plugins add features, each one isolated from the rest of the app. |
| <img src="docs/assets/icons/droplet.svg" width="20" alt=""> | **Light or dark.** One clean look, in light or dark, following Windows by default. |
| <img src="docs/assets/icons/calendar.svg" width="20" alt=""> | **Calendar.** Month, week and day views next to your mail, reminders, and calendars you follow by link. |
| <img src="docs/assets/icons/check-circle.svg" width="20" alt=""> | **Tasks.** Lists, subtasks and reminders, typed in one line, with the conversations they came from one click away. |

## Install

<img src="docs/assets/icons/download.svg" width="20" alt=""> Download `iris-setup-<version>.exe` from
[**Releases**](https://github.com/Alicienn/iris/releases/latest) and run it.

- No administrator rights needed.
- If Windows shows "Windows protected your PC", click *More info*, then *Run anyway*.
  The installer is not signed yet.
- To uninstall, go to *Settings › Apps*. Your mail and settings are kept.

## Getting started

1. Click **+** beside *Accounts* in the accounts column.
2. Enter your address and password. Iris finds the server settings on its own and tells
   you where it found them.
3. Your mail arrives, inbox first.

Your host or IT department sent a configuration profile (`.mobileconfig`, made for
iPhones and Macs)? **Import a profile** in the same window fills in the servers, and the
password when the profile carries one; check them and save. Signed profiles are read
too. A profile with several accounts fills in the first; POP accounts are refused,
since Iris speaks IMAP.

Passwords are stored in the Windows credential store.

### Tags for your mailboxes

With many mailboxes, tag them (*Clients*, *Personal*, *Club*). **Manage tags**, at the
bottom of the accounts column, creates, renames, recolours and deletes them, and you
drag a tag by its handle to change their order; right-click an account and open
**Tags** to tick its own, or type to find or create one.

The accounts are listed under their tags, in that order. Click a tag's name to see the
mail of its mailboxes only; click its arrow to fold its mailboxes away. The tag button
beside *Accounts* shows them all in one list instead.

## Getting around

The rail on the left goes from one part of Iris to another: the **Iris mark** at the top
for Home, then **Mail**, **Calendar** and **Tasks**, each with what waits there. At its
foot, search and commands (`Ctrl`+`K`) and the settings (`Ctrl`+`,`).

Iris is light or dark: *Settings › Appearance* follows Windows (the default), or keeps
one of the two.

## Home

Iris opens on **Home**: the date, a greeting with your first name (set it in
*Settings › Your first name*), one sentence on what is waiting, and **Next**: the coming
event or the next task with an hour. It opens where it lives; a task's circle ticks it
off. Below, **Mail**, **Tasks** and **Calendar** lead to each with what waits there.

Click the Iris mark at the top left, or press `Ctrl`+`0`, to come back to it. To open on
your mail instead, switch off *Open Iris on Home* in *Settings*.

## Mail

One column on the left holds **New message**, your accounts (grouped by tag, each tag
with its colour and what its mailboxes have to do) and, under them, the folders. Its
foot says when the mail last synced.

The list shows two lines per conversation, under the search box, the queues (**To do**,
**Waiting**, **Done**) and the **Unread**, **Attachments** and **Starred** filters.
Point at a conversation for **Done**, **Snooze** and **Archive** without opening it.
Choose how tight the list is in *Settings › Density*.

The reading pane's toolbar names its actions with their keys: **Done** `E`, **Snooze**
`S` (which asks until when: later today, tomorrow morning, this weekend, next week),
**Waiting** `W`, **To task** `T`; then star, archive, delete, **Read full screen**
(`Escape` to come back) and **More** (forward, read or unread, the source). Each message
of the conversation sits in its own frame, up to 900 pixels wide, its attachments under
it as file cards.

*Settings › Rules and plugins* opens the modules.

## Back and forward

The back and forward buttons of your mouse, the two arrows next to the window buttons,
or `Alt`+`←` and `Alt`+`→` go back to the places you visited (a mailbox, a folder, a tab,
a list of tasks, the calendar) and forward again, as in a browser.

## Calendar

Switch between **Mail** and **Calendar** in the rail, or with `Ctrl`+`1` and
`Ctrl`+`2`. The calendar opens on the week, then on the view you chose last.

- **New event** creates one in your own calendar; it can repeat and remind you before
  it starts.
- In the week or the day, click a free half hour and type the event's name: `Enter`
  adds it (an hour long, in your first calendar), `Escape` drops it. Double-click a
  slot for the full editor.
- Drag an event of your own to another hour or day, by quarter hours; drag its lower
  edge to change its length. Repeating events and subscribed calendars stay put.
- *To plan* lists today's tasks (and late ones) that have no hour yet. Drag one onto
  the week: it is booked there for as long as it takes (half an hour when not said),
  and the task gets that day and hour. Moving that slot later moves the task too.
- *Still today*, at the foot of the column, lists what is left of the day.
- **+** next to *Calendars* adds a calendar of your own besides *Personal*; each event
  is put in the one you choose. Right-click a calendar to rename it, change its colour
  or delete it.
- Click an event and its details open beside it; opened from Home or a task, they open
  in the middle of the window.
- Open an event to add **tasks** for it. They appear in *Tasks*, due when the event
  starts, and stay there if the calendar is hidden or removed.
- **Subscribe to a calendar…** follows a calendar published as a link: the `webcal://`
  or `.ics` address that Google Calendar, Outlook, iCloud, a school or a club gives out.
  Iris reads it every half hour; subscribed calendars are read-only.

Right-click a calendar to refresh, rename, recolour or delete it. Open an event to add
your own notes to it, even in a subscribed calendar. Opening an invitation received by
mail adds it to your calendar.

In the calendar: `T` today, `N` new event, `M` / `W` / `D` month, week, day, and the
arrow keys move to the previous or next period.

## Tasks

Open **Tasks** in the rail, or with `Ctrl`+`3`.

- Type a task in one line in the field at the bottom and press `Enter`. Iris reads the
  date, the time, the list and the priority out of it: `tomorrow 9am Call Marie #Work
  !!`, `vendredi 14h30 Dentiste`, `in 2 weeks Renew the passport`. `!`, `!!`, `!!!` go
  from low to high priority, shown as a coloured tag; a `#List` that does not exist yet
  is created. What Iris understood shows beside the field before you press `Enter`.
- **Today** shows what is due or late, under the date and what your calendars hold that
  day, with how many of the day's tasks are done. **Upcoming**, **All tasks** and
  **From mail** show the rest.
- The foot of the column counts what you finished this week, a bar a day.
- Drag a task onto a list or onto **Today** in the column on the left to move it there.
- A checked task stays, struck through, in the view it was checked from; **Clear**
  beside *Done* removes them.
- A task can hold subtasks, a note and a reminder, and say how long it takes (*Takes*:
  15 min, 30 min, 1 h, 2 h, or any length written out, like `1h20`).
- **Find a slot**, in a task's details, lists the free stretches of its day between your
  events and other timed tasks; the one you pick is blocked in your first calendar and
  gives the task its hour. *Remove* takes it back off.
- **Later** (`L`, or the arrow on the chosen row) puts a task off to tomorrow, this
  weekend, next week, a date, or someday. Iris counts it: a task put off three times
  asks whether to split it into steps, give it a slot, or let it go.
- In the mail, `T` (or *Add to tasks* in a conversation's right-click menu) turns the
  conversation into a task. *Open message* in the task brings it back.

### Goals

A goal is something to reach by a day: *Send 10 internship applications by the 17th*.
**+** beside *Goals* in the column on the left creates one, measured by a number or by
milestones. Its page shows where you stand, the pace you need ("6 to go in 18 days:
about 2 a week") and whether you are on track; **Log one** counts a step (with a word
on what you did, if you like), or tick the milestones as you reach them. What you add
on a goal's page is a step toward it; any task can be given a goal in its details.
**Make time for it** blocks time in your calendar on the days you choose, every week
until the goal's day. Today points to the goals behind or due this week. Iris counts
nothing on its own. Click a goal's title to rename it (`Enter` keeps it, `Escape` the
old one); the pencil beside it changes the rest: its target, its day, what it is for.
How a goal is measured stays as it was set.

### Repeating tasks and the week

*Repeats* in a task's details makes it come back: every day, every weekday, every
week, every month or every year. Ticked, it makes its next one, due on the next day of
its rule. Its hour, reminder, length and goal come with it, not its subtasks. A task
done late comes back after today, never in the past.

*This week*, in the column on the left, looks back and ahead: what is late, what was
put off, what is due next week, what was done since Monday, and the goals under way.

In Tasks: `N` new task, `J` / `K` or the arrows to move, `Space` done, `D` due date,
`L` later,
`Delete` delete, `Ctrl`+`Z` to bring back what you just deleted.

## Sending

Sending closes the message window at once. For a few seconds, a notice at the bottom
offers **Undo**, which brings the message back exactly as it was. Choose how long in
*Settings* (5 seconds by default, 0 to turn it off).

**Save draft** puts an unfinished message in the account's *Drafts* folder. Closing a
message you have started asks whether to keep it as a draft or discard it.

## Updates

Iris checks for a new version at launch and every few hours. When one is out, an
**Update now** button appears in the status bar: one click, one confirmation, and Iris
installs it and reopens. You can also check by hand in *Settings › Updates*.

**Changelog**, next to it, lists what changed in each version.

Every update is signed by the project. Iris checks the signature and the installer's
fingerprint before running anything, and refuses an update that does not match. The
check reads a small file from the release page rather than GitHub's API, so it works
on school and office networks where many people share one address.

## Keyboard shortcuts

| Key | Action |
|---|---|
| `E` | Mark as done |
| `A` | Archive |
| `S` | Snooze until tomorrow morning |
| `W` | Move to Waiting |
| `R` | Mark as read or unread |
| `F` | Star |
| `Shift`+`3` | Delete |
| `C` | New message |
| `/` or `Ctrl`+`F` | Search the mail |
| `Escape` | Leave full-screen reading, a search, a selection |
| `Ctrl`+`K` | Command palette |
| `T` | Add the conversation to tasks |
| `Ctrl`+`0` | Home |
| `Ctrl`+`1` / `Ctrl`+`2` / `Ctrl`+`3` | Mail / Calendar / Tasks |
| `Alt`+`←` / `Alt`+`→` | Back / Forward |
| `F5` | Sync all accounts |
| `Ctrl`+`,` | Settings |

Hover over any button to see its shortcut.

## Current limits

- Windows only for now.
- IMAP accounts with a password work out of the box. For Gmail, use an
  [app password](https://myaccount.google.com/apppasswords). Browser sign-in (Gmail,
  Outlook) is not enabled in published builds yet.

## Privacy

Iris talks to your mail servers, to the addresses of the calendars you subscribe to,
and to GitHub to ask for the latest version. That last request carries no information
about your mail or your accounts.

## Reporting a problem

Open an [issue](https://github.com/Alicienn/iris/issues) describing what you were doing
and what happened. Never include a password or the content of a private message.

---

<sub>To contribute or to see how Iris is built, read
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Licensed under MIT or
Apache-2.0, at your option.</sub>
