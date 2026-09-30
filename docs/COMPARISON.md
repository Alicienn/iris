# Iris beside the mail clients people already use

Measured against Thunderbird, Outlook, Apple Mail and Gmail — the four a new user
arrives from. The question is not "what do they have" but "what would someone miss on
the first afternoon, and never forgive".

## What Iris already does, and mostly does differently

| | Iris |
|---|---|
| Accounts | Many, unified, colour-coded per mailbox |
| Threading | Across accounts, on References and Message-ID |
| Triage | A queue — To do / Waiting / Done — instead of a flat inbox; Done, Snooze and Archive on a row's hover, as Gmail has them |
| Snooze | Later today, tomorrow morning, this weekend, next week |
| Reading | Each message framed, a full-screen mode, attachments as file cards |
| Folders | Created, renamed and deleted on every mailbox at once |
| Search | `from:`, `is:unread`, free text, over a local index |
| Automation | Rules, plus WebAssembly modules in a sandbox with no network |
| Composing | To / Cc / Bcc, attachments, formatting, signatures, an undo whose delay you choose |
| Offline | Every action journalled and replayed; nothing waits for the network |
| Privacy | Remote images blocked by default, trackers named |
| Calendar | Month, week and day, an event typed straight into a free slot, events dragged and stretched, today's tasks dragged onto the week, subscriptions by link, invitations from mail, reminders, your own notes and tasks on any event |
| Tasks | One-line entry, lists, subtasks, reminders, repeating tasks, a review of the week, tasks made from conversations and events, undo |
| Goals | A target or milestones by a date, the pace needed, time blocked weekly in the calendar — none of the four mail clients has them |
| Drafts | Saved to the server's Drafts folder, asked for when a started message is closed |
| Home | A start page for the day: one sentence on what waits, the next three events and tasks |
| Navigation | Back and forward across mailboxes, folders, views and workspaces, mouse buttons included |
| Many mailboxes | Tags that group, fold, order and filter them |
| Account setup | Address and password, or a configuration profile (`.mobileconfig`) as Apple Mail takes it |

The queue is the point of the product, and none of the four has it. What follows is
about the ordinary things they all have and Iris does not.

## Missing, ordered by how soon it hurts (as of 3.10.1)

### 1. Signing in to Gmail or Outlook with the browser

Both work today with an app password, and the code for signing in with Google or
Microsoft is there, but no published build turns it on: it needs an OAuth client of
one's own. For a Gmail account, an app password is the step where most people stop.

### 2. Answering an invitation

An invitation shows as a banner over its message and goes into the calendar in one
click (3.10.0). What Outlook and Thunderbird also do is answer it: Accept, Maybe or
Decline, sent back to the organiser (iTIP `REPLY`).

### 3. Sending later

Gmail, Outlook and Apple Mail can hold a message until a chosen time. Iris holds one
only for the seconds of its undo.

### 4. Sorting is always by date

Thunderbird and Outlook sort by sender, size and subject. Useful for "who sent me that
big file", but rarer than the above, and a queue is chronological by nature.

### 5. One address per account

No aliases: a mailbox that receives for several addresses can only send from one.

### 6. No spell check

Expected in a composer, and it needs dictionaries shipped and a text engine that can
underline a run. Real work, not a corner to cut.

### 7. No templates, no reminder when nobody answers

Saved replies, and "tell me in three days if there is no answer", which the *Waiting*
queue half does already.

### 8. No encryption, no server-side filters, no vacation reply

PGP and S/MIME are projects. Sieve means speaking another protocol. A vacation reply
means sending mail without being asked, which the module sandbox refuses on purpose: it
belongs in the application or nowhere, and probably nowhere.

### 9. Windows only

The code builds and its tests pass on macOS in CI; what a Mac build still lacks is the
tray, notifications, the title bar, a font of its own, a signed `.app`, and an
updater for it. Android would be a second interface.

## Closed since the first version of this list

Signatures per account, quick filters over the list, the message source, emptying a
folder and marking it read, opening an attachment, a calendar with invitations as a
banner over their message (added, updated, cancelled), `mailto:` links, system
notifications, starting at login.
