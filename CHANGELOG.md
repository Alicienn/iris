# Changelog

What changed in each version of Iris, newest first. Iris shows this same list from the
**Changelog** button in its status bar.

## 4.15.0 — 2026-10-08

### New
- Notes: write in spaces of Markdown files, arranged in a tree you drag and drop.
- Formatting as you type and with shortcuts: headings, lists, checkboxes, callouts, colours, highlight, underline, strikethrough.
- Links between notes, with what mentions a note, its outline, tags and search.
- LaTeX formulas drawn in the note, with snippets for the usual symbols.
- Spreadsheets with formulas and formatting, opened from Excel or CSV, and shown inside a note.
- Flashcards in your notes, brought back when it is time to revise them.
- Take notes for an event straight from the calendar.
- Link a note to a conversation, an event or a task; a checkbox can become a task.
- Reading mode, focus mode, a note beside another, the graph of links and past versions.
- Save a note as a web page or print it as PDF.
- Home shows your last notes and the flashcards due.

## 4.14.1 — 2026-10-07

### Improved
- Point at a folder with the orange warning to see how many mailboxes have it.

### Fixed
- A folder made for some of your addresses no longer shows the orange warning.
- A switched-off mailbox no longer makes every folder show the orange warning.

## 4.14.0 — 2026-10-07

### New
- Create a folder for only some of your addresses: pick them in the New folder window, with search and Select all.
- A folder made for some addresses shows only their mail.

## 4.13.0 — 2026-10-07

### New
- Plan my day: today's tasks without an hour are laid into your free time in one click.
- Connect your calendars from iCloud, Fastmail, Nextcloud, Gmail and other servers: changes go both ways.

## 4.12.0 — 2026-10-07

### New
- Unsubscribe from a mailing list in one click, beside its sender.
- Your calendars, tasks and goals are backed up every day; restore one from Settings.
- Export your calendars and tasks as .ics files for other apps.

## 4.11.0 — 2026-10-07

### Improved
- New mail for the mailbox on screen shows as soon as its server announces it.
- A folder you create shows at once.
- More folder names are recognised: Pourriels, Junk Mail, Messages supprimés, Brouillon, with or without accents.
- Shared folders are synced only when you are subscribed to them.
- A reply finished in the new-message window stays in its conversation.
- Saving a draft again replaces its copy on the server, and sending removes it.

### Fixed
- An encrypted profile says so.
- Signing in with Microsoft checks the account is the address you typed.
- Another program on your computer can no longer interrupt a sign-in.
- Renewing a sign-in twice at once no longer signs you out.
- Folder changes no longer wait for mailboxes that are switched off.
- Archive no longer aims at a read-only "all mail" view outside Gmail.
- A first sync no longer stalls on mailboxes with large gaps in their numbering.
- A snoozed conversation that gets a reply comes back.
- A Gmail reply reopens a conversation you had finished.
- Filtering by mailbox no longer shows conversations that left it.
- Actions on folders with accented names, waiting from an older version, reach the server.
- A mark set after undoing a move goes to the right message.

## 4.10.0 — 2026-10-07

### Improved
- Archiving on a mailbox without an archive folder makes one.
- Undo takes back a whole action on several conversations at once.
- What the rules do in a sync is undone in one step, and leaves redo alone.
- Snoozing until tomorrow morning means your morning, from the S key too.
- Mail put back in the inbox on another device is to do again here.

### Fixed
- Undoing an action already on its way to the server undoes it there too.
- Archiving a reply no longer takes filed messages out of their folders.
- Dragging a conversation that is not checked moves it, not the checked ones.
- Renaming, deleting or creating folders reaches every mailbox that shows them.
- A Gmail label and another server's folder of the same name are kept apart.
- Acting on a folder you just renamed works before the next sync.
- The bin in use is chosen when a server has two.
- New folders go where your other folders are.
- Dropping on a folder the mailbox does not have says so.
- Labels with a dot in their name show whole.
- Mark all as read reaches every copy on the server.
- Changes still waiting for the server are no longer undone by the next sync.

## 4.9.0 — 2026-10-07

### Improved
- Back to inbox, or a drop on the inbox, takes a conversation out of the bin or the junk.
- Outlook, Hotmail and Live addresses of every country sign in with Microsoft.
- Continue with Google works for Google Workspace domains behind a mail filter.
- Replying moves the conversation to Waiting, as the setting says.
- Send later checks the addresses and the attachments' size at once.
- A message on its way when Iris closes unexpectedly comes back as a draft.
- Sending works through office relays that need no sign-in.
- Forwarding keeps the message's attachments, with your signature above the forwarded text.
- A server that stops answering holds the others up two minutes at most.
- Several Gmail accounts no longer slow each other down.

### Fixed
- Actions a server refuses are tried again, and you are told if they are given up.
- Deleting a Gmail label no longer puts its mail back in the inbox.
- Undo still works once the mail has synced.
- Mail you take out of the junk stays out, here and on your other devices.
- Replies on Gmail quote the whole message, never a draft.
- A reply to a conversation sent to two of your addresses leaves from the one you used.
- Editing an account keeps its own sending login, and a new password works for sending too.
- An account whose server stops answering during a sync says so.
- Signing in with Microsoft no longer fails after you consent.
- A refused Office 365 sign-in offers to sign in again.
- Profiles keep their own sending login and password, address given or not.
- Adding an account gives up after 30 seconds instead of waiting for ever.
- The add-account screen starts encrypted every time.
- Picking another account of a profile no longer reuses the previous password.
- A scheduled message being sent can no longer be taken back and sent twice.
- A scheduled message the server refuses waits for you, not retried every five minutes.
- You are told when a sent message's copy could not go into Sent.
- A forward or a mail link no longer writes over the message in the window: it is kept aside.
- An invitation answer that could not be sent can be given again.
- Two replies to a message you do not have make one conversation.
- Search finds every new message, and deleted mail leaves it.
- Notifications count each new message once, and only what reaches the inbox.
- A mailbox is never synced twice at the same time.
- Mail another app marked deleted no longer counts as unread.
- Rules look at every new message once, and not again when it moves.
- Your actions reach the server in the order you made them.
- A message read again keeps its attachment and tracker marks.

## 4.8.0 — 2026-10-07

### New
- Emptying a folder asks first.

### Improved
- Moves happen in one step, and changes made elsewhere show sooner, on servers that allow it.
- Quitting while a message is on its way waits for it to leave.

### Fixed
- Accented letters in your messages show correctly for everyone you write to.
- A dropped connection no longer empties folders.
- Moving mail no longer purges messages another app had only marked deleted.
- Messages a server sends only in part are fetched one by one, not skipped.
- A message Iris cannot read stays in the list instead of vanishing.
- A message that fails while Iris quits comes back, and Iris stays open.
- A scheduled message sent while quitting no longer goes again.
- Actions waiting on a folder the server rebuilt no longer touch the wrong mail.
- Servers that do not give their next message number now sync.
- Renaming a folder to a name already taken says so.

## 4.7.2 — 2026-10-07

### Fixed
- The window keeps its rounded corners when restored from full size.

## 4.7.1 — 2026-10-07

### Improved
- Replies and forwards are introduced in English, as the rest of Iris is.
- Attachments that together pass 25 MB are refused when added, not after sending.

### Fixed
- The time in a quoted reply is your own, not UTC.
- Your Sent copy keeps the people you copied in Bcc.
- Removing an account also removes its mail from search.
- A folder you named "dovecot" is no longer hidden.

## 4.7.0 — 2026-10-07

### Improved
- The list shows a preview of each message as soon as it arrives.
- A first sync brings the newest mail first.
- New mail is looked for at least every 15 minutes, every 2 for the mailbox on screen.
- Replies to mail sent to an alias leave from that alias.
- Folders of the same name on different mailboxes show together.
- Folders created in Iris show in other mail apps too.

### Fixed
- HTML newsletters no longer look as if they carried an attachment.
- Mail the server marks as spam in its headers goes to Spam.
- One unreadable message no longer stops a folder from syncing.
- Marking read or starring right after archiving reaches the server.
- A conversation moved back to the inbox returns to To do.
- An archived or deleted conversation no longer comes back from a snooze.
- Unstarring a conversation removes every star in it.
- Checked conversations are unchecked when you change tab, search or mailbox.
- A bulk action that cannot be done says why.
- Empty and Mark all as read act only on the mailbox you are looking at.
- Deleting a folder no longer stops halfway across mailboxes.
- Signing in to Google as someone other than the address typed is refused.
- An old Google or Microsoft sign-in is renewed instead of failing.
- A company certificate installed on Windows is trusted for reading mail too.
- A DNS answer can no longer send your password to a server outside your domain.
- Microsoft 365 no longer gets two copies of what you send.
- Contacts whose name contains a comma can be picked as recipients.
- Drafts keep their Bcc and the alias you chose.
- Scheduled messages with large attachments no longer slow the window.
- Removing a mailbox says how many scheduled messages go with it.
- Two different messages sharing an identifier no longer merge.

## 4.6.0 — 2026-10-07

### New
- Accounts whose login is not their address sign in, with a new "User name" field.
- A Google or Microsoft account whose sign-in expired offers "Sign in again".

### Improved
- Undoing an archive or a delete undoes it on the server too.
- Archiving leaves your sent replies in Sent, and keeps Gmail labels and stars.
- A conversation moved to another folder keeps its state (Done, Waiting, snoozed).
- A new message in a conversation marked done brings it back to To do.
- Replies go to the Reply-To address, and Reply all includes the people in copy.
- Google Workspace addresses on your own domain are recognised.
- Profiles with logins, a sending password or SSL on an unusual port work.

### Fixed
- Servers on port 143 (STARTTLS) connect.
- Adding an existing account again no longer deletes its password.
- Editing an account no longer changes another account on the same server.
- Messages read or starred elsewhere now show so on servers such as Exchange.
- Reading a junk message no longer brings it back into To do.
- A mailbox that lost its connection syncs again on its own.
- A disabled mailbox stops syncing.
- A stuck download no longer blocks a mailbox.
- Emptying the bin or junk deletes for good instead of everything coming back.
- Moving mail no longer leaves a duplicate on servers without MOVE.
- Folders with accented names show and work.
- New and renamed folders follow each server's hierarchy, Gmail labels included.
- Gmail's unread count counts each message once.
- Mark all as read on Gmail clears conversations at once.
- Gmail's Starred and Important no longer show every message twice.
- Answers to invitations are recorded by Outlook and Google calendars.

## 4.5.3 — 2026-10-07

### Fixed
- A message that could not be sent says why and comes back, instead of "Message sent."
- A scheduled message stays scheduled until it has really left.
- Quitting Iris no longer loses a message you have just sent.
- Undo at the last second no longer lets the message leave anyway.
- A message keeps the mailbox it was written from after a restart.
- Replying after your own last message goes to your correspondent, not to you.
- Reply all includes the other people the message was sent to.
- Marking a message read, unread, then read again always reaches the server.
- Actions taken during a network drop are retried instead of lost.
- A folder deleted elsewhere no longer stops your other actions from reaching the server.
- A removed or failing mailbox no longer holds back the others' actions.
- Deleting a folder moves all its mail to the inbox, even mail not yet downloaded.
- Renaming or deleting a folder acts on that folder, not another with the same name.
- A folder named "Trash" or "Spam" inside another folder is no longer taken for the bin.
- iCloud's "Sent Messages" and "Deleted Messages" folders are recognised.
- A message can no longer show another message's text after the server rebuilds a folder.

## 4.5.2 — 2026-10-07

### Fixed
- Choosing a large mailbox such as Gmail no longer freezes Iris for seconds.

## 4.5.1 — 2026-10-07

### Fixed
- Iris stays responsive while a large mailbox syncs for the first time.
- Gmail conversations show once, not twice (inbox and All Mail).

## 4.5.0 — 2026-10-07

### Improved
- Each message is sent through the mailbox it is written from, aliases included.

### Fixed
- Mailboxes signed in with Google or Microsoft can send mail.
- Messages you send are kept in the mailbox's Sent folder.

## 4.4.1 — 2026-10-07

### Fixed
- Gmail accounts signed in with Google sync instead of failing.
- A message moved to the bin on another device leaves Inbox at the next sync.

## 4.4.0 — 2026-10-01

### New
- A new icon: mail, calendar and tasks as three arcs of one iris, in the app, the taskbar, the notification area and the installer.

## 4.3.0 — 2026-10-01

### New
- Choose an accent colour in Settings: blue, purple, pink, red, orange, yellow, green or graphite.

### Improved
- Settings is a page beside the side column, its settings grouped with a coloured icon each, as on a Mac.

## 4.2.0 — 2026-10-01

### Improved
- Settings and search sit on the window's bottom edge, at the foot of the side column.
- The line under a message's header runs across the whole reading pane.
- Calendar events are rounded on all four corners.
- A conversation's subject is no longer bold in the list; the blue dot says it is unread.

### Fixed
- Home's To answer counts the conversations it lists, read or not, instead of saying 0.
- Home lists every mailbox, including those under a folded tag.
- A mailbox that stopped syncing shows a red mark on Home, and why when pointed at.
- The faces in To answer each take their sender's colour.
- With nothing more today, Up next shows tomorrow's first event.

### Removed
- The search field at the top of Home; search stays a click away at the foot of the side column.

## 4.1.1 — 2026-10-01

### Fixed
- Home, Calendar and Tasks no longer show a pale line around the window's edge.

## 4.1.0 — 2026-10-01

### Improved
- The window has rounded corners when it is not maximised.
- A message's header says whom it was written to, with a line under it.
- Calendars and the tasks to plan are listed with a dot of their colour.
- Goals have a round flag icon in their colour, like lists.
- The magnifier beside Accounts finds an account; no filter field shows until then.

### Fixed
- Starring a conversation no longer freezes the window for a moment.

### Removed
- The "done this week" card at the foot of the task lists.
- The line of the day's events under Today in Tasks.

## 4.0.0 — 2026-10-01

### New
- A new look inspired by Apple's own apps: light grey side columns, white pages, blue selections.
- Home shows widgets: what's next with its Join button, a goal's ring, today's tasks, mail to answer and your week.
- Each place has its side column, with Home, Mail, Calendar and Tasks switched at its top, Back and Forward beside them.
- Choose Mac or Windows window buttons in Settings.
- Tasks open on big tiles: Today, Upcoming, All tasks and From mail, each with its count.

### Improved
- Mail's actions, the To do, Waiting and Done tabs and search now sit in one toolbar above the list and the reader.
- Conversations read like Apple Mail: no frames around messages, a larger subject, the selected row in blue.
- The calendar marks today with a red disc and the current time in a red capsule; events are softer, with a coloured edge.
- A list of tasks has its title in its colour, with how many are left beside it.
- While a folder is open, it carries the selection instead of the account.
- Buttons, fields, check boxes and pop-up menus look like the Mac's; settings are grouped, with switches.
- Scroll bars show only while you scroll or point at them.
- The status bar no longer cuts the side column short.

## 3.15.0 — 2026-10-01

### New
- Drag a task onto another to reorder the list; dropped in another day, it moves to that day.

### Improved
- Ticking a task draws the tick and strikes its title through, instead of flipping at once.
- A new task slides into the list with a short glow.
- The star bounces when you star a conversation.
- The To do, Waiting and Done marker slides from tab to tab.
- The day's and goals' progress bars fill smoothly.
- The calendar's now point pulses once when it opens.
- A dropped event lands with a small bounce.
- Softer shadows, deeper the higher things float.
- Thin scroll bars that widen when pointed at.

### Fixed
- Send and Reply all no longer overlap in the reply box.

## 3.14.0 — 2026-10-01

### New
- Several messages can be minimised at once: each becomes a bar at the bottom right, kept as a draft, and New message opens a fresh one.
- Events can have a video call link; Meet, Teams, Zoom and Webex links are found on their own, and a Join button opens them from the event and from Home.
- Continue with Google in Add an account, once your Google client is set in Settings.
- Pointing at Send or Reply all shows who it goes to, To, Cc and Bcc apart.
- A tag shows "!" while one of its accounts fails to sync.
- Syncing one account says so in the status bar, then how it went.
- Select a message's words with the mouse and copy them with Ctrl+C.
- Inbox zero: an empty To do is celebrated, with the days in a row you got there.
- After adding a task, a small bubble asks how long it takes; Enter skips it.
- Later offers Tonight, 23:59.
- Press ? for the list of keyboard shortcuts.
- Pointing at "Inbox +2" names every folder the conversation is in.

### Improved
- Ctrl+K lists everything at once and narrows the list as you type.
- A minimised message rises and grows back into place in one movement.
- Stretching an event that shares its column keeps its width, and the events it reaches make room as you go.
- A goal's page is more compact: its pace sits beside where it stands, its log on a card.
- A goal's new title is kept when you click elsewhere, as with Enter.
- Tasks are cards in a centred column, and their details float beside them.
- A task's details show the essentials; More options holds the slot, reminder, repeat and list.
- Softer corners throughout, and windows, menus and cards ease in.

### Fixed
- Gmail accounts that failed with "did not answer within 30 seconds" sync again.
- Long lines in the changelog wrap instead of running off the edge.
- Today's number in the calendar is a round mark again.
- The New message key hint and the All accounts icon of the folded column are centred.
- Pointing at Next on Home no longer hides the line under it.

### Removed
- The Goal field in a task's details: steps are added from the goal's page.

## 3.12.1 — 2026-10-01

### Improved
- Iris opens full size.

### Fixed
- Opening some newsletters, often from search, no longer closes Iris.
- An invitation's Accept, Maybe and Decline no longer overlap.

## 3.12.0 — 2026-09-30

### New
- Sign in with Google or Microsoft through the browser: paste your OAuth client in Settings, then add the account.
- Send as: give a mailbox other addresses (right-click it, Send as…); they appear as senders when you write.
- A configuration profile holding several accounts lets you pick which one to add.

### Fixed
- A Gmail or Outlook account with an app password can be added without an OAuth client set up.

## 3.11.0 — 2026-09-30

### New
- Send later: the clock beside Send offers this evening, tomorrow morning or afternoon, and Monday morning.
- Scheduled, under the folders, lists what waits: change a message or send it now.
- Answer an invitation from its banner: Accept, Maybe or Decline, sent to the organiser, and your calendar follows.
- Sort the mail list by date, sender, subject or size.
- Ctrl+Z in the calendar takes back a move, a stretch, a deletion or a colour.

## 3.10.1 — 2026-09-30

### Improved
- To plan: a clock button on each task books the first free time today, without dragging.
- Screen readers name the task details' lists (Reminder, Repeats, List, Goal), read a goal's pace chart, say which folder a conversation is in and how an event can be moved.

## 3.10.0 — 2026-09-30

### New
- An invitation in a message shows as a banner over it: Add to calendar, or Open once it is there.
- The banner says when an invitation changed or was cancelled since, and brings the calendar up to date in one click.
- The folder a conversation is in shows beside its mailbox: Inbox, Spam, Trash, or a folder's name.
- Pills under the search field: Unread, Attachments, 7 days, 30 days, Best match.

### Improved
- Search results come newest first, by day, like the list; Best match puts the closest first.

### Fixed
- A conversation deleted from the search results leaves them.

## 3.9.1 — 2026-09-30

### Fixed
- A mailbox whose server stops answering no longer holds up syncing: it is given up after a while, marked with its red !, and tried again later.
- Several mailboxes sync at once, so one slow server no longer delays all the others.

## 3.9.0 — 2026-09-30

### New
- Right-click an event for its menu: open it, edit it, delete it, or give it a colour of its own.
- An event's own colour replaces its calendar's everywhere it shows, and survives a subscribed calendar's refresh.

## 3.8.1 — 2026-09-30

### Fixed
- Clicking an empty spot in Tasks, Calendar or Home no longer reaches the mail underneath (it could open New message).
- No more stray icons and shapes flickering over Tasks while you carry a task.
- An event's details open beside the event you clicked, wherever the day is scrolled.
- A task dropped on the week lands at the hour it was dropped on.

### Removed
- Still today, at the foot of the calendar's side column.

## 3.8.0 — 2026-09-30

### Improved
- Updates are signed by the project: Iris installs only an update whose signature it can check.
- Checking for updates no longer runs into GitHub's hourly limit, on any network.

## 3.7.1 — 2026-09-30

### Fixed
- Checking for updates works when GitHub turns the check away, as it does on busy school or office networks.

## 3.7.0 — 2026-09-30

### New
- Click a goal's title to rename it; the pencil beside it changes its target, day and why.
- Tasks can repeat: every day, weekday, week, month or year. Done, the next one appears.
- This week, in Tasks: what is late, put off, due next week and done since Monday, with your goals.

### Improved
- Goal cards share the width instead of running off the page.

### Fixed
- Signed configuration profiles, as schools and companies send them, can be imported.

## 3.6.0 — 2026-09-30

### New
- Drag an event to another hour or day in the week; drag its lower edge to make it longer or shorter.
- To plan, under the calendars: today's tasks without an hour, to drag onto the week, where they book their length.
- A task's slot moved in the calendar takes the task's day and hour with it.

## 3.5.0 — 2026-09-30

### New
- Add an account from a configuration profile (.mobileconfig): Import a profile fills in its servers.

### Improved
- The mail list is cut into days: Today, Yesterday, This week, Earlier.
- On a goal's page, the log moves behind a switch when a task's details are open, leaving the steps their room.

## 3.4.1 — 2026-09-30

### Improved
- The task list is redrawn: a flat list with the day's events in one line above it.

## 3.4.0 — 2026-09-30

### New
- Say how long a task takes: 15 min, 30 min, 1 h, 2 h, or any other length.
- Find a slot: Iris lists the free stretches of the day and blocks the one you pick in your calendar.
- Later (L): tomorrow, this weekend, next week, a date, or someday.
- A task put off three times asks whether to split it, give it a slot, or let it go.
- Task rows show their goal, how long they take and how often they were put off.

## 3.3.0 — 2026-09-30

### New
- Goals in Tasks: something to reach by a day, counted or in milestones.
- Each goal shows where you stand, the pace you need, and whether you are on track.
- Log a step with "Log one", or tick a milestone; a chart follows your pace.
- Tasks can move a goal forward: add steps on its page, or pick its goal in a task.
- "Make time for it" blocks time for a goal in your calendar, every week until its day.
- Today points to the goals that are behind or due this week.

## 3.2.0 — 2026-09-30

### New
- Click a free hour in the week and type the event's name: Enter adds it, Escape drops it.
- "Still today" under your calendars: what is left of the day, and when.

### Improved
- The calendar is redrawn: tinted events with a fine outline, past ones fainter.
- The small month tints the week you are looking at and shows busy days in bold.
- Day, Week and Month, from smallest to largest; today in blue.
- New event is an outlined button.

## 3.1.0 — 2026-09-30

### New
- Done, Snooze and Archive on each conversation when you point at it in the list.
- Snooze asks until when: later today, tomorrow morning, this weekend or next week.
- Waiting in the reading toolbar, for a conversation that waits on an answer.
- Read full screen: the accounts and the list step aside until you press Escape.

### Improved
- Each message of a conversation sits in its own frame, in a wider reading column.
- Attachments show under the message they came with, as file cards.
- The mail list is flatter and easier to scan, the chosen conversation tinted blue.
- The queues and filters above the list are small pills; the search box is wider.
- The accounts column: tags with their colour and count, and when mail last synced.
- Rules and plugins now open from Settings.

## 3.0.0 — 2026-09-30

### New
- A rail on the left takes you to Home, Mail, Calendar and Tasks, with what waits in each.
- Iris follows Windows' light or dark mode, or stays in the one you choose.
- Tell Iris your first name in Settings and Home greets you by it.

### Improved
- A cleaner look throughout: white and grey panels in light, graphite in dark, one blue for what to act on.
- Home is quieter: the day in one sentence, the one thing next, and a way into mail, tasks and calendar.
- Menus and the status bar are lighter.

### Removed
- The five themes: Iris now has one look, light or dark.

## 2.0.0 — 2026-09-29

### New
- Tasks show your day above the list: the date, and what your calendars hold today.
- See how far along you are: "2 of 5 done" at the top of Tasks.
- What you have done this week, day by day, at the foot of the task lists.
- As you type a task, Iris shows the date, list and priority it understood before you press Enter.
- Click an event in the calendar and its details open beside it.
- "To tasks" in the reading toolbar turns the conversation into a task.

### Improved
- A redesigned Iris: every screen drawn again, with the same buttons, lists and windows everywhere.
- Home keeps to the essentials: your day in a sentence, the next three things, and a way into mail, tasks and calendar.
- Folders sit under your accounts in one column, leaving more room for your mail.
- The mail list is more compact: two lines per conversation, with the sender's mailbox as a dot.
- The queues To do, Waiting and Done are the title of the list.
- The reading toolbar names its actions and shows their keys.
- The reply field stays one line until you write in it.
- Calendar: the week reads at a glance, with today in red, the weekend quieter and the current time in the margin.
- Tasks: rows on one plate, and the details of a task laid out as a list of properties.

### Removed
- The day dial and the daily quote on Home.

## 1.1.0 — 2026-09-29

### New
- Open a conversation straight from Home.

### Improved
- A new Home: a dial of your day with its events, one sentence on what is waiting, then your mail, today and your tasks side by side.
- Today's events on Home sit on a line down the hours, with the present marked.
- Icons are drawn with rounded strokes throughout Iris.

## 1.0.0 — 2026-09-29

### New
- Home: your day at a glance, opened from the name Iris at the top left or with Ctrl+0.
- Home shows your unread mail, what arrived today, tasks due and this week's events, each one click away.
- Iris opens on Home; switch it off in Settings to open on your mail.
- Back and forward between the places you visit, with your mouse buttons, the arrows by the window buttons, or Alt+Left and Alt+Right.
- Add tasks to a calendar event; they appear in Tasks, due when it starts.
- Click a tag in the accounts column to see the mail of its mailboxes only.
- Fold a tag's mailboxes away with its arrow.
- Drag tags into the order you want in their window.
- Ctrl+Z brings back a task you just deleted.

### Improved
- Accounts are grouped by tag from the start, and tag names read like folders.
- The accounts, folders, calendars and task lists look and behave alike.
- The calendar opens on the week, then on the view you chose last.
- New calendar is now a + next to Calendars.
- Deleting a task no longer opens the next one.

### Fixed
- Rounded windows, menus and panels no longer show a straight line across their corners.
- Text fields keep a single line's height.
- In compact density, the message excerpt is no longer cut off at the bottom.

## 0.8.0 — 2026-09-29

### New
- Save a message as a draft with the Save draft button; it goes to your Drafts folder.
- Closing a message you started asks whether to keep it as a draft.
- Drag a task onto a list, or onto Today, to move it there.
- Delete all completed tasks of a view in one click.

### Improved
- Iris uses much less memory while its window is open.
- The Cc/Bcc button stays, so the two fields can be folded again; a filled one stays open.
- Tasks: the add field sits at the bottom, cards are larger, and priority shows as a coloured tag.
- Tasks: Anytime becomes All tasks and lists every task, today's included.
- Tasks: Steps become Subtasks.
- Tasks: a checked task stays, struck through, in the view you checked it from.
- Tasks: Today no longer shows the Calendar and Mail to do sections.
- Clearer wording in messages across Iris.

### Fixed
- New message opens an empty message after a send.
- New message opens the message window even when one was minimised.

## 0.7.0 — 2026-09-29

### New
- Tag your mailboxes (Clients, Personal, Club…) from a Tags button at the bottom of the accounts column.
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
