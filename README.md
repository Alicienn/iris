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
| <img src="docs/assets/icons/sun.svg" width="20" alt=""> | **Home.** Your day at a glance: unread mail, what arrived, tasks due and this week's events, one click from each. |
| <img src="docs/assets/icons/inbox.svg" width="20" alt=""> | **A work queue.** Every conversation is *to do*, *waiting* or *done*. One keystroke moves it along. |
| <img src="docs/assets/icons/users.svg" width="20" alt=""> | **All your accounts in one place.** A unified inbox, or one account at a time, without slowing down. |
| <img src="docs/assets/icons/zap.svg" width="20" alt=""> | **Instant.** Opening, archiving and searching never make you wait, even with hundreds of thousands of messages. |
| <img src="docs/assets/icons/shield.svg" width="20" alt=""> | **Private.** Remote images and tracking pixels are blocked by default. No telemetry. |
| <img src="docs/assets/icons/layers.svg" width="20" alt=""> | **Modules.** Rules and plugins add features, each one isolated from the rest of the app. |
| <img src="docs/assets/icons/droplet.svg" width="20" alt=""> | **Themes.** Several looks included, dark and light. |
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

1. Click **+** at the top of the accounts column.
2. Enter your address and password. Iris finds the server settings on its own and tells
   you where it found them.
3. Your mail arrives, inbox first.

Passwords are stored in the Windows credential store.

### Tags for your mailboxes

With many mailboxes, tag them (*Clients*, *Personal*, *Club*). **Manage tags**, at the
bottom of the accounts column, creates, renames, recolours and deletes them, and you
drag a tag by its handle to change their order; right-click an account and open
**Tags** to tick its own, or type to find or create one.

The accounts are listed under their tags, in that order. Click a tag's name to see the
mail of its mailboxes only; click its arrow to fold its mailboxes away. The tag switch
above the list shows them all in one list instead.

## Home

Iris opens on **Home**: a greeting, a line to start the day, your unread mail, what is
waiting in your queue, what arrived today, the tasks due, and this week's events. Each
figure and each line opens where it lives; a task's circle ticks it off. **Sync all
accounts** at the top right fetches everything, and the Iris mark turns while it does.

Click **Iris** at the top left, or press `Ctrl`+`0`, to come back to it. To open on your
mail instead, switch off *Open Iris on Home* in *Settings*.

## Back and forward

The back and forward buttons of your mouse, the two arrows next to the window buttons,
or `Alt`+`←` and `Alt`+`→` go back to the places you visited (a mailbox, a folder, a tab,
a list of tasks, the calendar) and forward again, as in a browser.

## Calendar

Switch between **Mail** and **Calendar** in the title bar, or with `Ctrl`+`1` and
`Ctrl`+`2`. The calendar opens on the week, then on the view you chose last.

- **New event** creates one in your own calendar; it can repeat and remind you before
  it starts.
- **+** next to *Calendars* adds a calendar of your own besides *Personal*; each event
  is put in the one you choose. Right-click a calendar to rename it, change its colour
  or delete it.
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

Open **Tasks** in the title bar, or with `Ctrl`+`3`.

- Type a task in one line in the field at the bottom and press `Enter`. Iris reads the
  date, the time, the list and the priority out of it: `tomorrow 9am Call Marie #Work
  !!`, `vendredi 14h30 Dentiste`, `in 2 weeks Renew the passport`. `!`, `!!`, `!!!` go
  from low to high priority, shown as a coloured tag; a `#List` that does not exist yet
  is created.
- **Today** shows what is due or late. **Upcoming**, **All tasks** and **From mail**
  show the rest.
- Drag a task onto a list or onto **Today** in the column on the left to move it there.
- A checked task stays, struck through, in the view it was checked from; **Delete
  completed** clears them.
- A task can hold subtasks, a note and a reminder.
- In the mail, `T` (or *Add to tasks* in a conversation's right-click menu) turns the
  conversation into a task. *Open message* in the task brings it back.

In Tasks: `N` new task, `J` / `K` or the arrows to move, `Space` done, `D` due date,
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

## Keyboard shortcuts

| Key | Action |
|---|---|
| `E` | Mark as done |
| `A` | Archive |
| `S` | Snooze until tomorrow |
| `R` | Mark as read or unread |
| `F` | Star |
| `Shift`+`3` | Delete |
| `C` | New message |
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
