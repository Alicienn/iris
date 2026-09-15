# Modules: ten ideas, and the three worth building

Iris runs plugins as WebAssembly in a sandbox that grants only what the manifest asks
for and the host agrees to give. Secrets and account writes are never granted, network
access does not exist, and fuel and memory are bounded per call. That shape decides
what a good plugin *is* here: something that reads what it is shown and answers with an
action, quickly, and would be irritating to hard-code into the application because
everybody wants it slightly differently.

## The ten

**1. Sender rules by domain.** Everything from `@ovh.net` goes to Waiting. Trivially
useful, and already covered by the rules engine — a plugin would be a second answer to
a question the application answers.

**2. Working hours.** Mail arriving outside your hours is snoozed to the next morning.
Small, opinionated, and exactly the kind of preference nobody agrees on.

**3. Attachment sorter.** Anything with an invoice attached goes to a folder. Depends
on reading attachment names, which the host exposes.

**4. Thread ageing.** A conversation nobody has answered in N days comes back to the
top. Already in the follow-up automation.

**5. Newsletter digest.** Collect everything with a `List-Unsubscribe` header and mark
it read once a day, leaving one summary. Needs a scheduled trigger the host does not
have.

**6. VIP.** A named list of people whose mail is starred and never leaves the queue.
Two lines of logic, high daily value, and the list is different for everyone.

**7. Duplicate detector.** The same message delivered to three of your addresses shows
once. Needs cross-thread state the plugin API does not carry.

**8. Language router.** File by detected language. Fun, and a solution looking for a
problem for most people.

**9. Auto-responder.** Out of office. Needs to *send*, which means SMTP, which the
sandbox does not grant and should not.

**10. Read-time estimator.** Puts "2 min read" on long threads. Cheap, mildly useful,
purely decorative.

## The three I built, and why these

The choice follows the application's current shape, not the ideas' cleverness. Iris now
has folders that mean something, a queue that is about work, and a triage flow built
around "what still needs me". A plugin earns its place by making that flow shorter.

### `vip` — people who never get lost

*Idea 6.* Names a list of addresses and domains. Mail from them is starred, kept in the
queue whatever else happens, and never marked Waiting by another rule.

It is first because it is the one that fails badly when absent: everything else in the
application is about getting mail *out* of the way, and there is currently nothing that
says "not this one". Ten lines of logic; the value is entirely in the list being yours.

### `office-hours` — mail that waits until you are working

*Idea 2.* Anything arriving outside configured hours is snoozed to the next working
morning. Weekends included.

It is second because it is the clearest use of the settings mechanism built in the last
round: hours and days are exactly the kind of thing a plugin must ask and an application
must not decide. And it turns background sync from something that interrupts into
something that accumulates — which is the point of a queue.

### `filer` — attachments go where attachments belong

*Idea 3, sharpened.* Matches subject and attachment names against patterns and files the
thread into a folder. Configured as `pattern → folder` pairs.

It is third because folders only became real in this round: creating one creates it
everywhere, and dragging files a message into it. This is the automatic version of that
gesture, and without it a folder tree is a filing system you have to operate by hand.

## What they share

Each reads what the host already sends — sender, subject, attachment names, arrival
time — and answers with an action the host already knows how to perform. None asks for
a capability that is not already granted. That is not a coincidence: a plugin that
needs a new host capability is a feature request wearing a plugin's clothes, and it
belongs in the application or nowhere.

## Building them

```
cargo build --release -p iris-plugin-vip -p iris-plugin-office-hours -p iris-plugin-filer --target wasm32-unknown-unknown
```

The build script `packaging/build-plugins.ps1` does that and lays out each plugin's
directory — `plugin.toml` beside `plugin.wasm` — under `plugins/`, ready to be installed
from Modules → Add.
