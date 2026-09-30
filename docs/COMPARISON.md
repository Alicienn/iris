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
about the ordinary things they all have and Iris did not, when this was written.

> **Status (1.0.0):** items 1 to 5 are built, and so is most of item 7 — a calendar,
> and invitations opened from a message go into it. What remains of 7 is answering an
> invitation (Accept / Decline). Items 6, 8 and 9 are still missing.

## Missing, ordered by how soon it hurts

### 1. No signature — *every message Iris sends is unsigned*

All four have this, per account, since forever. For anyone using mail for work it is
noticed on the first message and cannot be worked around except by retyping four lines
each time. **Nothing else on this list is missed this quickly.**

### 2. No way to narrow the list — *unread only, with attachments, starred*

Every client has these, usually as a strip above the list. Iris has search, which
answers a different question: search is for finding one message you remember, a filter
is for reducing several hundred you have not read. On a queue of 188, the second is the
one you reach for.

### 3. No message source — *no headers, no raw text*

Thunderbird has Ctrl+U, Outlook and Apple Mail bury it in a menu. It matters less often
than the rest, and when it matters nothing else will do: a message that arrives wrong,
a sender who is not who they claim, a rule that fires when it should not.

### 4. No bulk folder actions — *no "mark all as read", no "empty the bin"*

The screenshot shows Trash at 865. Every client can empty it. Marking a folder read is
the other half: after a week away, opening two hundred messages one by one to clear a
badge is not triage.

### 5. Attachments can be saved but not opened

Saving then finding the file in Explorer is three steps where every other client has
one. The reading pane already lists them with size and type.

### 6. Sorting is always by date

Thunderbird and Outlook sort by sender, size and subject. Genuinely useful for "who
sent me that big file" — but rarer than the five above, and a queue is chronological by
nature.

### 7. Nothing for calendars or invitations

An `.ics` in a message is an attachment like any other. Outlook and Thunderbird show a
card with Accept / Decline. This is a large piece of work — a calendar store, a
timezone library, an iTIP reply path — and it is the honest answer for why it is not
next.

### 8. No spell check

Expected in a composer, and it needs dictionaries shipped and a text engine that can
underline a run. Real work, not a corner to cut.

### 9. No encryption, no aliases, no server-side filters, no vacation reply

PGP and S/MIME are projects. Aliases are small but only matter to people who have them.
Sieve means speaking another protocol. A vacation reply means sending mail without
being asked, which the module sandbox refuses on purpose — it belongs in the
application or nowhere, and probably nowhere.

## What was built next

Five — all shipped since — chosen because each is missed early, none needs a new subsystem, and together
they close the gap between "an interesting way to triage mail" and "a mail client you
can actually live in":

1. **Signatures**, per account, appended to what you write and to replies.
2. **Quick filters** over the list: unread, attachments, starred.
3. **Message source**, headers and all.
4. **Empty a folder** and **mark a folder read**.
5. **Open an attachment**, not only save it.
