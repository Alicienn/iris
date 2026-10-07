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
too, as are the logins and the separate sending password a profile may give. A profile
with several accounts offers a list to pick which one fills the fields; POP accounts
are refused, since Iris speaks IMAP.

**Configure manually** shows the servers and a **User name** field, for hosts and
companies whose login is not the address (`jdoe`, `DOMAIN\jdoe`); leave it empty to
sign in with the address. Port 143 is upgraded with STARTTLS; nothing is ever sent
unencrypted.

**Signing in with Google or Microsoft.** In *Settings › Sign in with Google or
Microsoft*, paste the details of an OAuth client for a desktop app. For Google, create
it in the Google Cloud Console (its client ID and secret). For Outlook, register an
app in Microsoft Entra (its application ID). Adding a Gmail or Outlook address then
opens the browser to sign in, with no password to type. **Continue with Google**, under
the address in *Add an account*, does it directly; it stays greyed, and says why when
pointed at, until a Google client is set. Without a client, type an app password as
the password. Google Workspace addresses on a domain of your own are recognised too.
When such a sign-in expires or is withdrawn, the mailbox's red **!** (or *Password* in
its menu) offers **Sign in again**.

A tag carries a red **!** while one of its mailboxes fails to sync, folded or not.
**Sync** on a mailbox says *Syncing …* in the status bar, then how it went.

**Send as.** Right-click a mailbox, *Send as…*, to give it the other addresses it may
send from (aliases the server knows, with a name of their own if you like). They
appear as senders when you write, after the mailbox's own address. A message leaves
through the outgoing server of the mailbox it is from, signed in as that mailbox (with
Google or Microsoft for those accounts); an alias goes through the mailbox it belongs to.

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

Each place has its side column, and at its top four buttons go from one to another:
**Home**, **Mail**, **Calendar** and **Tasks** (a dot when tasks are due). Back and
forward sit just above them. At the column's foot, the settings (`Ctrl`+`,`) and search
and commands (`Ctrl`+`K`: every command at once, narrowed as you type).

*Settings* opens as a page beside the side column, its settings in groups; **Done**
or `Escape` closes it. Iris is light or dark: *Settings › Appearance* follows Windows
(*System*, the default), or keeps one of the two. *Accent colour* tints selections,
buttons and links: blue (the default), purple, pink, red, orange, yellow, green or
graphite. *Window buttons* draws the window's close, minimise and maximise buttons as
the Mac's three lights at the top left, or as Windows' at the top right (the default
on Windows).

## Home

Iris opens on **Home**: the date, a greeting with your first name (set it in
*Settings › Your first name*), one sentence on what is waiting, then widgets:

- **Up next**: the coming event or the next task with an hour, with its Join button
  when it is held on a video call, and the events after it.
- **Goal**: the first goal under way, its ring and whether it is on track.
- **Today**: the tasks due today or late; a circle ticks one off.
- **To answer**: the first conversations of To do, and how many wait for an answer.
- **This week**: the tasks done, a bar a day, and your inbox zero streak.

Each widget's title opens its place, each line what it names. Click **Home** at the top
of the side column, or press `Ctrl`+`0`, to come back to it. To open on your mail
instead, switch off *Open Iris on Home* in *Settings*.

## Mail

The side column holds your accounts (grouped by tag, each tag with its colour and what
its mailboxes have to do) and, under them, the folders. The magnifier beside *Accounts*
opens a field to find one by its address. Its foot says when the mail last synced.

One toolbar runs above the list and the reader: where you are and how much is to do,
the queues (**To do**, **Waiting**, **Done**), **New message** (`C`), what acts on the
open conversation, and search (`/`). The list shows two lines per conversation, under
the **Unread**, **Attachments** and **Starred** filters.
Point at a conversation for **Done**, **Snooze** and **Archive** without opening it.
Choose how tight the list is in *Settings › Density*.

The toolbar names its actions with their keys: **Done** `E`, **Snooze**
`S` (which asks until when: later today, tomorrow morning, this weekend, next week),
**Waiting** `W`, **To task** `T`; then star, archive, delete, **Read full screen**
(`Escape` to come back) and **More** (forward, read or unread, the source). The messages
of the conversation follow one another, a hairline between them, up to 900 pixels wide,
their attachments under them as file cards. Under the subject, beside the mailbox, the folder the conversation is
in: *Inbox*, a folder's name, or *Spam* and *Trash* in red; *Inbox +2* names all of
them when you point at it.

Iris looks for new mail at least every quarter of an hour, every two minutes for the
mailbox on screen; **Sync** (`F5` for all) asks at once. A mailbox added for the
first time brings its newest mail first.

**Archive** takes a conversation out of the inbox and leaves your sent replies in
*Sent*; on Gmail it removes the *Inbox* label only, so labels and stars stay. **Delete**
moves it to the bin. Undo (`Ctrl`+`Z`) puts it back on the server too, and a
conversation moved to another folder keeps its state (*Done*, *Waiting*, snoozed). A
new message in a conversation you had marked done brings it back to *To do*, unless
*Settings › A new message reopens the thread* is off. **Back to inbox** (`u`), or
dropping a conversation on the inbox, takes it out of the bin or the junk folder, and
tells the server it is not junk. **Empty** on the bin or the junk
folder deletes for good, once you have said yes. Quitting while a message is still on
its way waits for it to leave; if it cannot, Iris stays open with your message.

Select a message's words with the mouse as on any page, and copy them with `Ctrl`+`C`.

An empty *To do* is **Inbox zero**: Iris says so, and counts the days in a row you got
there.

A message carrying an invitation (an `.ics`) shows it as a banner at its top: what,
when, and **Accept**, **Maybe** or **Decline**. The answer is sent to the organiser
from the mailbox the invitation came to, and your calendar follows: added for Accept
and Maybe, taken out for Decline. The banner then says what you answered; another
click changes it. An invitation with no organiser to answer offers **Add to calendar**
instead. Once added, it says *In your calendar* and offers **Open**. If the organiser
changed or cancelled it since, it says so and brings the calendar up to date in one
click. An event is never added twice: invitations are matched by their identifier.

The list is by date, newest first; the button at the end of the filters sorts it by
sender, subject or size (the biggest message first) instead, for the three queues.

Minimise a message you are writing and it becomes a bar at the bottom right, kept as a
draft on this computer (even if Iris closes); **New message** then starts another, and
a click on a bar brings its message back. In the reply box under a conversation, point
at **Send** or **Reply all** to see who it goes to.

The clock beside **Send** sends later: this evening (before five), tomorrow morning or
afternoon, or Monday morning. The message waits in Iris, so Iris must be running when
its time comes. *Scheduled*, under the folders while something waits, lists them:
change one (it comes back into the composer and waits no more), or send it now.

Searching shows pills under the field: **Unread**, **Attachments**, **7 days**,
**30 days**, **Best match**. A pill puts its words in the query (`is:unread`,
`has:attachment`, `newer_than:7d`, `sort:relevance`), so what it does can be typed too.
Results come newest first, grouped by day; *Best match* puts the closest first.
A conversation deleted from the results leaves them.

*Settings › Rules and plugins* opens the modules.

## Back and forward

The back and forward buttons of your mouse, the two arrows at the top of the side column,
or `Alt`+`←` and `Alt`+`→` go back to the places you visited (a mailbox, a folder, a tab,
a list of tasks, the calendar) and forward again, as in a browser.

## Calendar

Switch between **Mail** and **Calendar** at the top of the side column, or with `Ctrl`+`1` and
`Ctrl`+`2`. The calendar opens on the week, then on the view you chose last.

- **New event** creates one in your own calendar; it can repeat and remind you before
  it starts.
- In the week or the day, click a free half hour and type the event's name: `Enter`
  adds it (an hour long, in your first calendar), `Escape` drops it. Double-click a
  slot for the full editor.
- Drag an event of your own to another hour or day, by quarter hours; drag its lower
  edge to change its length. Repeating events and subscribed calendars stay put.
- An event held on a video call shows **Join on Meet** (or Teams, Zoom, Webex) in its
  card and on Home. Links in an invitation are found on their own; paste one in the
  card's *Video call link* to add or change it, in any calendar.
- Stretch an event that shares its time with others and they make room as you go.
- Right-click an event for its menu: *Open*, *Edit…*, *Delete* (in your own calendars),
  and its colour. A colour chosen there is the event's own, in place of its calendar's,
  kept even for a subscribed calendar; *Use the calendar's colour* gives it back.
- `Ctrl`+`Z` in the calendar takes back the last move, stretch, deletion or colour,
  then the one before (twenty at most).
- *To plan* lists today's tasks (and late ones) that have no hour yet. Drag one onto
  the week: it is booked there for as long as it takes (half an hour when not said),
  and the task gets that day and hour. Moving that slot later moves the task too. The
  clock that shows on a row when you point at it does the same without dragging: the
  first free time today.
- **+** next to *Calendars* adds a calendar of your own besides *Personal*; each event
  is put in the one you choose. Right-click a calendar to rename it, change its colour
  or delete it.
- Click an event and its details open beside it; opened from Home or a task, they open
  in the middle of the window.
- Open an event to add **tasks** for it. They appear in *Tasks*, due when the event
  starts, and stay there if the calendar is hidden or removed.
- **Subscribe to a calendar…** (the link next to *Calendars*) follows a calendar published as a link: the `webcal://`
  or `.ics` address that Google Calendar, Outlook, iCloud, a school or a club gives out.
  Iris reads it every half hour; subscribed calendars are read-only.

Right-click a calendar to refresh, rename, recolour or delete it. Open an event to add
your own notes to it, even in a subscribed calendar. Opening an invitation received by
mail adds it to your calendar.

In the calendar: `T` today, `N` new event, `M` / `W` / `D` month, week, day, and the
arrow keys move to the previous or next period.

## Tasks

Open **Tasks** at the top of the side column, or with `Ctrl`+`3`. Four tiles open
**Today**, **Upcoming**, **All tasks** and **From mail**, each with its count; your
lists and goals follow.

- Type a task in one line in the field at the bottom and press `Enter`. Iris reads the
  date, the time, the list and the priority out of it: `tomorrow 9am Call Marie #Work
  !!`, `vendredi 14h30 Dentiste`, `in 2 weeks Renew the passport`. `!`, `!!`, `!!!` go
  from low to high priority, shown as a coloured tag; a `#List` that does not exist yet
  is created. What Iris understood shows beside the field before you press `Enter`.
- Once added, a small bubble over the field asks how long it takes: click a length or
  type one (`45m`, `1h20`) and press `Enter`; `Enter` alone skips it, and typing the
  next task simply adds it.
- Tasks are cards in a column of a readable width; a task's details float beside them.
  They show what matters (due, length, priority, steps, notes); *More options* shows
  the slot, reminder, repeat and list.
- **Today** shows what is due or late, under the date and what your calendars hold that
  day, with how many of the day's tasks are done. **Upcoming**, **All tasks** and
  **From mail** show the rest.
- The foot of the column counts what you finished this week, a bar a day.
- Drag a task onto a list or onto **Today** in the column on the left to move it there.
  Drag it onto another task to put it above or below; a line shows where. Dropped in
  another day, it takes that day. Within a day, tasks with an hour come first, by
  time, then the others in the order you gave them.
- A checked task stays, struck through, in the view it was checked from; **Clear**
  beside *Done* removes them.
- A task can hold subtasks, a note and a reminder, and say how long it takes (*Takes*:
  15 min, 30 min, 1 h, 2 h, or any length written out, like `1h20`).
- **Find a slot**, in a task's details, lists the free stretches of its day between your
  events and other timed tasks; the one you pick is blocked in your first calendar and
  gives the task its hour. *Remove* takes it back off.
- **Later** (`L`, or the arrow on the chosen row) puts a task off to tonight (by
  23:59), tomorrow, this weekend, next week, a date, or someday. Iris counts it: a task put off three times
  asks whether to split it into steps, give it a slot, or let it go.
- In the mail, `T` (or *Add to tasks* in a conversation's right-click menu) turns the
  conversation into a task. *Open message* in the task brings it back.

### Goals

A goal is something to reach by a day: *Send 10 internship applications by the 17th*.
**+** beside *Goals* in the column on the left creates one, measured by a number or by
milestones. Its page shows where you stand, the pace you need ("6 to go in 18 days:
about 2 a week") and whether you are on track; **Log one** counts a step (with a word
on what you did, if you like), or tick the milestones as you reach them. What you add
on a goal's page is a step toward it.
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
*Settings › Undo send* (5 seconds by default; up to 30, or off).

*Message sent.* appears once the server has taken the message. If it refuses it, or
cannot be reached, Iris says why and puts the message back: in the window it was
written in, or, if that window now holds another message, at the bottom right. A
scheduled message stays under *Scheduled* until it has left, and one that could not
leave is tried again five minutes later. Quitting Iris waits for messages already sent
to leave.

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
| `Ctrl`+`C` | Copy the words selected in a message |
| `?` | The list of keys |

Hover over any button to see its shortcut, or press `?` for all of them.

## Current limits

- Windows only for now.
- IMAP accounts with a password work out of the box. For Gmail, use an
  [app password](https://myaccount.google.com/apppasswords), or sign in through the
  browser once you have set an OAuth client of your own in *Settings › Sign in with
  Google or Microsoft*. Iris ships without one: each provider wants its own
  registration.
- A message's attachments can weigh 25 MB together, the limit of most servers.
- Replies and forwards are introduced in English ("On …, … wrote:").

## Privacy

Iris talks to your mail servers, to the addresses of the calendars you subscribe to,
and to GitHub to ask for the latest version. That last request carries no information
about your mail or your accounts.

Removing an account removes from this computer its mail, its passwords and its words
in the search index.

## Reporting a problem

Open an [issue](https://github.com/Alicienn/iris/issues) describing what you were doing
and what happened. Never include a password or the content of a private message.

---

<sub>To contribute or to see how Iris is built, read
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Licensed under MIT or
Apache-2.0, at your option.</sub>
