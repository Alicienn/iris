# Changelog

What changed in each version of Iris, newest first. Iris shows this same list from the
**Changelog** button in its status bar.

## 0.7.0 — 2026-09-29

### New
- Tag your mailboxes — Clients, Personal, Club… — from a Tags button at the bottom of the accounts column.
- Right-click an account to tick its tags, with a search that can also create one.
- Group the accounts by tag with the switch above their list.
- Create calendars of your own besides Personal, and pick one for each event.

### Improved
- A cleaner icon for Modules.

### Fixed
- Right-clicking another account while a menu is open now opens that account's menu.

### Removed
- The Add modules window: modules are still installed by placing them in their folder.

## 0.6.0 — 2026-09-28

### New
- Write your own notes on any calendar event, subscribed calendars included.
- Change a calendar's colour from its right-click menu.
- Choose how long you can undo a send, in Settings (5 seconds by default).

### Improved
- Sending closes the message window at once; a notice at the bottom lets you undo and brings the message back as it was.

### Fixed
- Conversations in Done can be deleted.
- The list density choices in Settings no longer spill out of their frame.
- The Updates section of Settings scrolls into view entirely.

## 0.5.0 — 2026-09-28

### New
- Tasks, next to your mail and calendar (Ctrl+3): lists, steps, notes, due dates and reminders.
- Type a task in one line: "tomorrow 9am Call Marie #Work !!" sets the date, time, list and priority.
- Today gathers what is due or late, today's events and the conversations still to do.
- Press T, or right-click a conversation, to turn it into a task that opens the message again.
- Upcoming, Anytime and From mail views, and lists you can create, rename and delete.

## 0.4.0 — 2026-09-28

### New
- Right-click a calendar to refresh, rename or delete it.
- Pick an event's dates in a small month instead of typing them.

### Improved
- The month view shades the days of the months before and after, and names each first of the month.
- An event opened from the calendar shows its whole title.
- Deleting or unsubscribing from a calendar asks first.
- Escape closes a calendar window even while you are typing in it.

### Fixed
- Replies no longer show their text printed over the message they quote.
- Signatures and newsletters no longer show black boxes around their text.
- Text in a signature is no longer covered by the picture that follows it.

## 0.3.0 — 2026-09-28

### New
- Calendar, next to your mail: month, week and day views (Ctrl+2).
- Create events, repeat them and get a reminder before they start.
- Subscribe to a calendar by its link (webcal or .ics from Google, Outlook, iCloud…).
- Opening an invitation received by mail adds it to your calendar, and keeps it up to date.

### Improved
- Newsletters open using a fraction of the memory, and very long ones show in full.
- Much less memory used while Iris waits in the notification area.
- The window itself uses about five times less memory.
- Newsletters stay sharp at any window size.
- Each sender gets a round mark with their initials, and unread mail a dot.
- New message opens with the cursor in the To field.
- Text fields keep the same background whether or not you are typing in them.

### Fixed
- Opening Iris while it waits in the notification area brings its window back.
- Times in the message list are shown in your time zone.
- Clicking another field moves into it on the first click.
- Clicking a suggested address fills it in.
- Shortcuts no longer act on the mail behind an open window.

## 0.2.0 — 2026-09-27

### New
- Changelog button in the status bar: see what changed in each version.
- Iris checks for updates at launch and installs them in one click.
- Check for updates by hand from Settings.

### Improved
- A certificate error names the server to use instead.
- Changing a password or an account now says whether it works.

### Fixed
- An account that fails to sync shows a red ! with the reason.
- A message opened for the first time no longer shows an empty page.
- Sync, modules and settings buttons fit when the account list is collapsed.
- Folders you delete no longer come back.
- Internal server folders (dovecot, sieve) are no longer listed.

## 0.1.0 — 2026-09-20

### New
- First release: every account in one window, with a unified inbox.
- To do, waiting and done queues, with snooze and follow-ups.
- Full-text search across all mail.
- Remote images and tracking pixels blocked by default.
- Rules, modules and themes.
