# Iris Growth — design

**Date:** 2026-10-09 · **Status:** approved by the owner ("implement it, as close to the
web mockup as possible; no journal for now; no mail cut-off") ·
**Mockup:** `..\Iris-design\croissance\index.html` (`#home`, `#habits`, `#review`,
`#compass`) · **Ships as:** 5.0.0 onwards, one release per phase.

A sixth place in Iris, beside Home, Mail, Calendar, Tasks and Notes: **Growth**, for who one
wants to become rather than what there is to do. Tasks is doing; Growth is the habits,
the goals, the balance between the areas of one's life, and the review that closes the
week.

## 1. Stances

- **Nothing is counted on the user's behalf.** A habit is ticked by hand, a score is
  given by hand, a win is written by hand. Iris adds up what was ticked.
- **A missed day never piles up.** A habit is only ever about today: there is no
  backlog of habits, no red count of what was not done.
- **Local and private**, as the rest: the base on this computer, in the daily backup.
- **Out of scope for now:** the journal (morning intention, evening close, energy and
  mood), cutting the mail off at night, focus sessions.

## 2. The place

- The switcher gets a sixth segment, a sprout, after Notes: `Ctrl`+`5`, workspace 5.
- Its side column (`Nav`, `NavItem`, `SectionRow`, as every other):
  - **Today**: *Habits* (`2 of 4`), and from phase 3 *Wins*.
  - **Compass** (phase 2): the cycle under way (`Cycle 3 · week 5/12`).
  - **Goals**: the goals, moved from the Tasks column, with their title leading to all
    of them and **+** to make one.
  - **Reviews** (phase 3): *This week*, the month, the year so far.
- **Goals move here.** Their page (where it stands, the steps toward it, the log) is the
  same component as before; only the column around it changes. The Tasks column loses
  its *Goals* section; a task still names its goal. A goal's page is drawn by the tasks'
  own view with its column hidden, so that steps, the add bar and a step's details stay
  one piece of code.
- Back and forward know the place: `growth` with `habits`, `goals`, `goal:{id}`
  (`Place::after`), and from later phases `compass`, `review`, `wins`.

## 3. Habits (phase 1, 5.0.0)

### Data (migration 25)

```
habits(id, title, icon, color, kind 'build'|'quit', schedule, amount, unit,
       goal_id → goals ON DELETE SET NULL, remind_minute, reminded_day,
       position, created_at, archived_at)
habit_checks(habit_id → habits ON DELETE CASCADE, day 'YYYY-MM-DD', amount,
             PRIMARY KEY(habit_id, day)) WITHOUT ROWID
```

- `schedule`: `daily`, `days:MTWTFSS` as seven `0`/`1` (Monday first), or `week:N`
  (N times a week, any days).
- `amount`/`unit`: 0 and empty for a plain habit; "20" "pages" for one with an amount.
  A check holds the amount done that day; less than the amount is *partial*.
- `kind`: *build* (do more of) or *quit* ("Less of": ticked when the day was kept).
- Both tables are in the daily backup and its restore.

### Rules (`iris-growth`, pure)

- A day of a habit is *due* when its schedule says so (`week:N`: every day is possible,
  N are wanted in the week). A day not due is a rest day.
- **Streak**, for `daily` and `days:`: due days done in a row, counted back from today;
  today not yet done does not break it, a due day missed does, rest days are skipped,
  a partial day breaks it. For `week:N`: weeks in a row with N days done, the current
  week counting once it is reached.
- **Best streak**: the longest run ever. **Rate**: done due days out of due days over
  the last 30 days (for `week:N`, done days out of N × weeks, capped at 100 %).
- A cell of a day is *done*, *partial*, *missed*, *rest*, *to do today*, *future*, or
  *before the habit existed*.
- One-line entry: `Read 20 pages every day`, `Run 3 times a week`, `Spanish 15 min on
  weekdays`, `No phone in bed every day` (a leading *no*/*stop*/*less* makes it a
  *Less of* habit). What is not understood stays in the name; the icon is guessed from
  the words (book, run, drop, chat, phone…), and can be changed.

### Screens (as `#habits`)

- **Toolbar**: *Habits*, `2 of 4 today`; *Week · Month · Year*; **+ New habit**.
- **Page**: *This week* and its dates, a line under it; habits grouped *Every day*,
  *Some days*, *Less of*, each group with the day letters (today in the accent) and
  *Streak* / *This week* at the right. A row: icon tile in its colour, name, what it is
  ("20 pages a day · Read 12 books"), a circle per day (filled in its colour when done,
  half-filled when partial, a dash on a rest day, a ring in the accent for today still
  to do, faint for the days to come), the streak with a flame. Clicking a circle ticks
  that day (today or before); space ticks today for the chosen row. *Month* shows the
  month's days as small circles; *Year* the twelve months, each shaded by its rate.
- An **add line** at the foot reads a habit typed in one line.
- **Details** (`DetailPanel`, right): icon and name, a sentence, three figures (days in
  a row, best streak, last 30 days), the last six months as a grid of days, the goal it
  supports (a click opens it), its schedule and reminder; *Edit* and *Delete*.
- **New habit / Edit habit** (`Modal`): name, icon and colour, *Do more · Do less*, how
  often (*Every day · Some days · Times a week*, with the days or the number), an
  amount and its unit, the goal it supports, a reminder hour.
- **Reminder**: at its hour, a habit due today and not done yet is notified once.
- **Home**: a *Habits* widget, `2 of 4`, a disc per habit of today (a click ticks it),
  and the longest streak under way as its foot.
- Keys in Growth: `N` new habit, `J`/`K` and the arrows move, `Space` ticks today,
  `Escape` closes the details.

## 4. Compass (phase 2, 5.1.0)

- **Areas of life**: six to start (Studies & career, Health, Relationships, Mind, Money,
  Fun), each with a colour and an icon; renamed, added, removed. A goal, a habit and a
  task list may belong to one (migration: `areas`, `area_id` on goals, habits and task
  lists).
- **Scores**: once a month, each area scored 1–10 by hand (`area_scores`, by month).
  The **wheel** draws this month's scores over last month's (a filled path over a
  dashed one; stroked paths only).
- **Time**: per area, this month's hours — tasks done in its lists or for its goals
  (their length, half an hour when not said) and calendar time blocked for its goals —
  against the hours wanted, set per area. A bar with a mark at the wanted hours.
- **Cycles of twelve weeks**: a name, a start, the goals chosen for it (`cycles`,
  `cycle_goals`). The header shows twelve segments, the week under way, and *what you
  planned, done*: the steps of the cycle's goals done out of those due so far.
- Screen as `#compass`: the cycle strip, the wheel, a card per area (score, its goals
  and habits, its time bar); *Areas · Vision · Cycles* (Vision: a page of one's own
  words; Cycles: past and next cycles); *Score this month*.

## 5. Review and wins (phase 3, 5.2.0)

- **Wins**: a line written by hand, dated (`wins`); a goal reached and a milestone
  ticked are offered as wins, never added on their own. *Wins* in the column lists
  them by week.
- **Weekly review**, as `#review`, five steps in a column at the left, each skippable:
  1. *Clear the decks*: To do, Waiting, the tasks late.
  2. *Look back*: tasks done (against the week before), habits kept, goal steps logged,
     the wins of the week, what was put off three times or more (*Slot*, *Let go*).
  3. *Goals and habits*: where each stands.
  4. *Next week's three*: three tasks chosen, then given time.
  5. *One line to keep*: what was learned, kept with the review (`reviews`).
  The Tasks tab's *This week* view gives way to it.
- **Today's three** on Home: three tasks of the day chosen as the ones that matter,
  the first marked *Hard one first*; set from Today or from the review.

## 6. What changes elsewhere

- `README.md`: a *Growth* section, `Ctrl`+`5`, the keys of Growth, Home's widget.
- `docs/ARCHITECTURE.md`: `iris-growth` in the domain layer, the place, migrations
  25–27, the reminder timer.
- Version: 5.0.0 (goals move out of the Tasks column: a habit broken), then 5.1.0 and
  5.2.0.
