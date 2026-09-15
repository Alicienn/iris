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

It is also the only one of the three that ships **off**. The other two are inert until
you write something into them: an empty list of people names nobody, an empty list of
rules files nothing. This one has plausible hours from the start, so without a switch it
would begin taking mail out of the view on the evening it was installed. Snoozing is the
one sorting action that removes a message before anyone has seen it, which is exactly
the kind of thing that should wait to be asked for.

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
directory — `plugin.toml` beside `plugin.wasm` — under `packaging/plugins/`, ready to be
installed from Modules → Add. The installer copies that tree into
`%APPDATA%\Iris\plugins`, which is the one directory the application reads.

## What running them actually took

The host had never run a plugin compiled from Rust. Its example is hand-written
WebAssembly, and every difference between the two turned out to be a defect — each one
silent, which is why they had all survived:

- **Reference types were switched off**, in the belief that they were shared threads.
  Since rustc 1.82 the `wasm32-unknown-unknown` target encodes indirect calls that way,
  so every module built from Rust was refused outright. The message named a formatting
  routine inside `core` and said "zero byte expected", which leads nowhere.
- **The results buffer was one slot wide.** A Rust function that returns nothing returns
  nothing; wasmtime rejected the call before running it, with "expected 0 results, got
  1" — a message that seems to accuse the module while describing the host.
- **The instance was rebuilt on every call.** Whatever a module learned at `iris_init`
  was gone by the first message. Nothing errored: the modules loaded, initialised, ran,
  and did nothing whatsoever. This was the worst of the four, because it is invisible
  from both sides.
- **The SDK's case-insensitive search allocated** a lowercased copy of the haystack per
  call, on a bump heap that never frees — six hundred copies for one message with thirty
  rules and twenty attachments.

Keeping the instance alive raises the question that the per-call instance had been
answering by accident. A heap that never frees runs out; a module would have stopped
being able to receive a payload after a few hundred messages, mid-afternoon, with
nothing to explain why sorting had stopped. The SDK now exports `iris_reset`, which the
host calls before every event and never before `iris_init`: what a module files away at
startup stays, and what it allocates for one message leaves with it.

`crates/iris-plugins/tests/livres_wasm.rs` is what found all four, and is the only thing
that could. It loads the real compiled binaries, dispatches payloads of the shape the
host actually composes, and pushes two thousand messages through a single instance.
The unit tests in each crate check the decision — who matters, when the day starts,
where an invoice goes. Everything above lives in the space between those tests and the
running application.
