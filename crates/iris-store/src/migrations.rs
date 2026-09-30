//! Schéma et migrations.
//!
//! Les migrations sont une liste ordonnée et immuable : une migration publiée n'est
//! jamais modifiée, seulement suivie d'une autre. La version est portée par
//! `PRAGMA user_version`, qui vit dans l'en-tête du fichier et ne coûte donc aucune
//! table supplémentaire.

use iris_types::{Error, Result};
use rusqlite::Connection;

pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Version courante du schéma.
pub const CURRENT_VERSION: i64 = 19;

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial schema",
        sql: SCHEMA_V1,
    },
    Migration {
        version: 2,
        name: "rules",
        sql: SCHEMA_V2,
    },
    Migration {
        version: 3,
        name: "backfill spam",
        sql: SCHEMA_V3,
    },
    Migration {
        version: 4,
        name: "take binned mail out of the queue",
        sql: SCHEMA_V4,
    },
    Migration {
        version: 5,
        name: "repair the moves that could never replay",
        sql: SCHEMA_V5,
    },
    Migration {
        version: 6,
        name: "the bin is a folder, not a state",
        sql: SCHEMA_V6,
    },
    Migration {
        version: 7,
        name: "a thread remembers which mailbox it came from",
        sql: SCHEMA_V7,
    },
    Migration {
        version: 8,
        name: "each mailbox gets a signature",
        sql: SCHEMA_V8,
    },
    Migration {
        version: 9,
        name: "calendars",
        sql: SCHEMA_V9,
    },
    Migration {
        version: 10,
        name: "tasks",
        sql: SCHEMA_V10,
    },
    Migration {
        version: 11,
        name: "put aside, event notes",
        sql: SCHEMA_V11,
    },
    Migration {
        version: 12,
        name: "account tags",
        sql: SCHEMA_V12,
    },
    Migration {
        version: 13,
        name: "tags in an order of one's own, tasks of an event",
        sql: SCHEMA_V13,
    },
    Migration {
        version: 14,
        name: "goals, and how long a task takes",
        sql: SCHEMA_V14,
    },
    Migration {
        version: 15,
        name: "tasks that come back",
        sql: SCHEMA_V15,
    },
    Migration {
        version: 16,
        name: "an event's own colour",
        sql: SCHEMA_V16,
    },
    Migration {
        version: 17,
        name: "mail sent later",
        sql: SCHEMA_V17,
    },
    Migration {
        version: 18,
        name: "addresses a mailbox sends as",
        sql: SCHEMA_V18,
    },
    Migration {
        version: 19,
        name: "video call links of events",
        sql: SCHEMA_V19,
    },
];

/// The video call an event is held on (Meet, Teams, Zoom, Webex…), set by hand. Beside
/// the events and not in them, by calendar and UID as their colours are, so a link
/// given to a subscribed event outlives its refresh. One found in the event's own place
/// or description needs no row: it is read from there.
const SCHEMA_V19: &str = "
CREATE TABLE event_links (
    calendar_id INTEGER NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    uid         TEXT NOT NULL,
    url         TEXT NOT NULL,
    PRIMARY KEY (calendar_id, uid)
);
";

/// The other addresses a mailbox sends as (aliases): the server accepts them from
/// that account, and the composer offers them as senders. A name of their own when
/// they want one ("Support"), else the account's.
const SCHEMA_V18: &str = "
CREATE TABLE account_aliases (
    id         INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    address    TEXT NOT NULL,
    name       TEXT NOT NULL DEFAULT '',
    UNIQUE (account_id, address)
);
";

/// Messages written now to leave later. The draft is kept, not a composed message: it
/// is composed when its time comes (with that date), and one taken back opens in the
/// composer as it was written. `payload` is its JSON, attachments included.
///
/// And the answers given to invitations (Accept, Maybe, Decline), by the event's UID,
/// so the banner over the invitation says what was answered.
const SCHEMA_V17: &str = "
CREATE TABLE scheduled_mail (
    id         INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    send_at    INTEGER NOT NULL,
    to_line    TEXT NOT NULL DEFAULT '',
    subject    TEXT NOT NULL DEFAULT '',
    payload    TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX scheduled_mail_by_time ON scheduled_mail(send_at);

CREATE TABLE invite_replies (
    uid        TEXT PRIMARY KEY,
    partstat   TEXT NOT NULL,
    replied_at INTEGER NOT NULL
);
";

/// An event can wear a colour of its own instead of its calendar's. Kept beside the
/// events rather than in them, by calendar and UID as the notes are: a subscribed
/// calendar's events are replaced at each refresh, and the colour chosen for one must
/// outlive that. Every occurrence of a repeating event takes it.
const SCHEMA_V16: &str = "
CREATE TABLE event_colors (
    calendar_id INTEGER NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    uid         TEXT NOT NULL,
    color       TEXT NOT NULL,
    PRIMARY KEY (calendar_id, uid)
);
";

/// A task can repeat: `repeat` names how ("daily", "weekdays", "weekly", "monthly",
/// "yearly"), NULL when it does not. Done, it makes its next one, due on the next day
/// the rule gives; the one done stays done, in the log of what was done.
const SCHEMA_V15: &str = "
ALTER TABLE tasks ADD COLUMN repeat TEXT;
";

/// Goals, and three things a task learns.
///
/// **A goal** is something to reach by a day: a number of something ("10 internship
/// applications") counted by hand in its log, or a few milestones ticked in order. Its
/// pace is read from the day it was set, its due day and where it stands.
///
/// **A task** can move a goal forward (`goal_id`; the goal deleted, the task stays),
/// say how long it takes (`estimate`, minutes) and count how many times it was put off
/// (`postponed`), so that one moved again and again can be asked about.
const SCHEMA_V14: &str = "
CREATE TABLE goals (
    id         INTEGER PRIMARY KEY,
    title      TEXT NOT NULL,
    why        TEXT NOT NULL DEFAULT '',
    kind       TEXT NOT NULL DEFAULT 'count',
    target     INTEGER NOT NULL DEFAULT 1,
    unit       TEXT NOT NULL DEFAULT '',
    due_day    TEXT NOT NULL,
    color      TEXT NOT NULL,
    position   INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

CREATE TABLE goal_entries (
    id      INTEGER PRIMARY KEY,
    goal_id INTEGER NOT NULL REFERENCES goals(id) ON DELETE CASCADE,
    at      INTEGER NOT NULL,
    note    TEXT NOT NULL DEFAULT ''
);
CREATE INDEX goal_entries_by_goal ON goal_entries(goal_id, at);

CREATE TABLE goal_milestones (
    id       INTEGER PRIMARY KEY,
    goal_id  INTEGER NOT NULL REFERENCES goals(id) ON DELETE CASCADE,
    title    TEXT NOT NULL,
    position INTEGER NOT NULL DEFAULT 0,
    done_at  INTEGER
);
CREATE INDEX goal_milestones_by_goal ON goal_milestones(goal_id, position);

ALTER TABLE tasks ADD COLUMN goal_id INTEGER REFERENCES goals(id) ON DELETE SET NULL;
ALTER TABLE tasks ADD COLUMN estimate INTEGER;
ALTER TABLE tasks ADD COLUMN postponed INTEGER NOT NULL DEFAULT 0;
CREATE INDEX tasks_by_goal ON tasks(goal_id) WHERE goal_id IS NOT NULL;
";

/// Two additions.
///
/// **Tags have an order**, the one the user drags them into; the accounts column
/// groups them in it. Existing tags start in the alphabetical order they were shown in.
///
/// **A task can belong to an event**: its identifier and the start of the occurrence
/// (0 for an event that does not repeat, so that moving it keeps them). Not the
/// calendar: a task outlives the calendar being hidden, unsubscribed or deleted.
const SCHEMA_V13: &str = "
ALTER TABLE account_tags ADD COLUMN position INTEGER NOT NULL DEFAULT 0;
UPDATE account_tags SET position = (
    SELECT COUNT(*) FROM account_tags AS avant
    WHERE avant.name COLLATE NOCASE < account_tags.name COLLATE NOCASE
) + 1;

ALTER TABLE tasks ADD COLUMN event_uid TEXT;
ALTER TABLE tasks ADD COLUMN event_start INTEGER;
CREATE INDEX tasks_by_event ON tasks(event_uid, event_start) WHERE event_uid IS NOT NULL;
";

/// Les tags des adresses.
///
/// Des étiquettes qu'on pose sur ses propres boîtes — « Clients », « Perso »,
/// « Association » — pour les retrouver et les regrouper dans la colonne des comptes.
/// Une boîte en porte autant qu'on veut. Le nom est unique sans égard à la casse :
/// « Clients » et « clients » seraient deux groupes que personne ne distingue.
const SCHEMA_V12: &str = "
CREATE TABLE account_tags (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE COLLATE NOCASE,
    color      TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE account_tag_links (
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    tag_id     INTEGER NOT NULL REFERENCES account_tags(id) ON DELETE CASCADE,
    PRIMARY KEY (account_id, tag_id)
);

CREATE INDEX account_tag_links_by_tag ON account_tag_links(tag_id);
";

/// Deux ajouts.
///
/// **Un fil mis à la corbeille le quitte tout de suite.** Supprimer déplace les
/// messages par le journal, que le serveur rejoue plus tard ; jusque-là ils sont
/// toujours, localement, dans leur dossier. Un fil déjà « fait » ne changeait donc pas
/// d'état et restait affiché : dans l'onglet Done, supprimer semblait ne rien faire.
/// `put_aside_at` retire le fil des files à l'instant, et s'efface si un message plus
/// récent arrive.
///
/// **Les notes d'un événement.** Les siennes, pas celles de l'événement : un calendrier
/// abonné est remplacé en bloc à chaque relecture, et ses événements ne se modifient
/// pas. Rangées à part, par calendrier, identifiant et début d'occurrence — une note
/// sur la réunion de lundi n'est pas celle du lundi suivant. Elles partent avec le
/// calendrier ; masquer puis réafficher le calendrier les retrouve.
const SCHEMA_V11: &str = "
ALTER TABLE threads ADD COLUMN put_aside_at INTEGER;

CREATE TABLE event_notes (
    calendar_id      INTEGER NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    uid              TEXT NOT NULL,
    occurrence_start INTEGER NOT NULL,
    notes            TEXT NOT NULL,
    updated_at       INTEGER NOT NULL,
    PRIMARY KEY (calendar_id, uid, occurrence_start)
);
";

/// Les tâches.
///
/// Des listes, et des tâches dans les listes. Une sous-tâche est une tâche qui a un
/// parent ; elle part avec lui. L'échéance est un jour (`YYYY-MM-DD`, lu tel quel,
/// sans fuseau : « demain » est demain où que l'on soit) et, si elle en a une, une
/// heure en minutes depuis minuit. Le rappel est l'instant où le donner, calculé à
/// l'écriture.
///
/// Une tâche venue du courrier garde le fil d'où elle vient, et de quoi le nommer
/// sans aller le relire : le fil peut disparaître, la tâche reste.
///
/// Une liste « My tasks » existe d'emblée, pour que la première tâche ait où aller.
const SCHEMA_V10: &str = "
CREATE TABLE task_lists (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    color      TEXT NOT NULL,
    position   INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

CREATE TABLE tasks (
    id            INTEGER PRIMARY KEY,
    list_id       INTEGER NOT NULL REFERENCES task_lists(id) ON DELETE CASCADE,
    parent_id     INTEGER REFERENCES tasks(id) ON DELETE CASCADE,
    title         TEXT NOT NULL,
    notes         TEXT NOT NULL DEFAULT '',
    due_day       TEXT,
    due_minute    INTEGER,
    remind_before INTEGER,
    remind_at     INTEGER,
    reminded      INTEGER NOT NULL DEFAULT 0,
    priority      INTEGER NOT NULL DEFAULT 0,
    done_at       INTEGER,
    thread_id     INTEGER,
    source        TEXT NOT NULL DEFAULT '',
    position      INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);

CREATE INDEX tasks_by_list ON tasks(list_id, done_at);
CREATE INDEX tasks_by_parent ON tasks(parent_id);
CREATE INDEX tasks_by_reminder ON tasks(remind_at) WHERE reminded = 0 AND done_at IS NULL;
CREATE INDEX tasks_by_thread ON tasks(thread_id) WHERE thread_id IS NOT NULL;

INSERT INTO task_lists (name, color, created_at) VALUES ('My tasks', '#5b8def', 0);
";

/// L'agenda.
///
/// Deux tables. Un calendrier est **local** — ses événements se créent et se modifient
/// ici — ou **abonné** : il a une adresse, ses événements sont remplacés en bloc à
/// chaque relecture, et on ne les modifie pas. Un événement récurrent est une ligne
/// portant sa règle : ses occurrences se déroulent à l'affichage.
///
/// Les exclusions d'une règle sont une liste d'instants séparés par des virgules : elles
/// ne s'interrogent jamais, elles se relisent avec l'événement.
///
/// Un calendrier « Personal » existe d'emblée, pour que « New event » ait toujours où
/// aller.
const SCHEMA_V9: &str = "
CREATE TABLE calendars (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL,
    color         TEXT NOT NULL,
    source_url    TEXT,
    visible       INTEGER NOT NULL DEFAULT 1,
    etag          TEXT,
    last_modified TEXT,
    last_sync     INTEGER,
    last_error    TEXT,
    created_at    INTEGER NOT NULL
);

CREATE TABLE calendar_events (
    id               INTEGER PRIMARY KEY,
    calendar_id      INTEGER NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    uid              TEXT NOT NULL,
    summary          TEXT NOT NULL DEFAULT '',
    description      TEXT NOT NULL DEFAULT '',
    location         TEXT NOT NULL DEFAULT '',
    start_ms         INTEGER NOT NULL,
    end_ms           INTEGER NOT NULL,
    all_day          INTEGER NOT NULL DEFAULT 0,
    tzid             TEXT,
    rrule            TEXT,
    exdates          TEXT NOT NULL DEFAULT '',
    recurrence_id    INTEGER,
    cancelled        INTEGER NOT NULL DEFAULT 0,
    reminder_minutes INTEGER,
    updated_at       INTEGER NOT NULL
);

CREATE INDEX calendar_events_by_time ON calendar_events(start_ms, end_ms);
CREATE INDEX calendar_events_by_calendar ON calendar_events(calendar_id);

INSERT INTO calendars (name, color, created_at) VALUES ('Personal', '#5b8def', 0);
";

/// Une signature par compte.
///
/// Tout client de courrier en a une depuis toujours ; Iris envoyait chaque message non
/// signé, et la seule parade était de retaper quatre lignes à chaque fois. Par compte
/// et non globale, parce que c'est la raison d'avoir plusieurs comptes : on ne signe pas
/// une facture comme on écrit à sa sœur.
///
/// Vide par défaut. Personne ne veut découvrir une signature inventée par le programme
/// au bas d'un message déjà parti.
const SCHEMA_V8: &str = r#"
ALTER TABLE accounts ADD COLUMN signature TEXT NOT NULL DEFAULT '';
"#;

/// Donne à chaque fil le compte de son dernier message.
///
/// La liste dessine une pastille de couleur à gauche de chaque ligne, dont tout
/// l'intérêt est de dire de quelle boîte vient le message quand on les regarde toutes
/// ensemble. Elle était calculée à partir de l'adresse du **premier compte**, la même
/// pour toutes les lignes : cent boîtes, une seule couleur, et un repère qui affirmait
/// quelque chose de faux plutôt que de ne rien dire.
///
/// La colonne rejoint les autres colonnes dénormalisées du fil, pour la raison qui les
/// a toutes mises là : une page de liste doit se servir en une seule lecture d'index,
/// sans jointure. Le remplissage initial prend le compte du message le plus récent de
/// chaque fil — exactement ce que `refresh_thread` maintiendra ensuite.
const SCHEMA_V7: &str = r#"
ALTER TABLE threads ADD COLUMN last_account_id INTEGER NOT NULL DEFAULT 0;

UPDATE threads
SET last_account_id = COALESCE((
        SELECT m.account_id
        FROM messages m
        WHERE m.thread_id = threads.id
        ORDER BY m.received DESC, m.id DESC
        LIMIT 1
    ), 0);
"#;

/// Défait la migration 4 : la corbeille n'est pas un état, c'est un endroit.
///
/// La migration 4 rangeait dans « Terminé » tout fil qui n'existait plus que dans une
/// corbeille. L'intention était bonne — ce courrier n'a rien à faire dans la file de
/// travail — et le moyen était faux. « Terminé » veut dire « je m'en suis occupé » ;
/// huit cent cinquante messages jetés y noyaient les quelques dizaines que
/// l'utilisateur avait réellement traités, et la file « Terminé » ne voulait plus rien
/// dire.
///
/// Ces fils sont donc rendus à l'état qu'ils avaient, et c'est la **requête** qui les
/// écarte désormais des trois files. La différence se voit : ils réapparaissent quand
/// on ouvre la corbeille, et nulle part ailleurs.
///
/// Rendre l'état perdu est possible sans l'avoir enregistré parce que la migration 4
/// ne touchait que `state = 0`. Un fil que l'utilisateur aurait lui-même marqué
/// terminé **et** qui serait entièrement à la corbeille serait ramené à tort — mais la
/// nouvelle requête le cache de toute façon, donc la correction ne se voit pas.
const SCHEMA_V6: &str = r#"
UPDATE threads
SET state = 0
WHERE state = 2
  AND EXISTS (SELECT 1 FROM messages WHERE messages.thread_id = threads.id)
  AND NOT EXISTS (
        SELECT 1
        FROM messages
        JOIN folders ON folders.id = messages.folder_id
        WHERE messages.thread_id = threads.id
          AND folders.role NOT IN ('trash', 'junk')
  );
"#;

/// Réécrit les opérations de déplacement que le rejeu n'a jamais su lire.
///
/// Le producteur écrivait `"to"`, le consommateur attendait `"target"`. Chaque
/// archivage et chaque suppression était donc jugé illisible, abandonné, et marqué
/// terminé sans avoir rien fait. Le fil quittait la file localement ; sur le serveur,
/// le message n'a jamais bougé.
///
/// La correction du code empêche d'en écrire de nouveaux. Celle-ci répare ceux qui
/// existent : le nom du champ est corrigé, et l'opération est remise en attente,
/// parce qu'aucune n'a jamais réussi — elles ont été abandonnées, pas exécutées.
/// L'utilisateur a demandé ces déplacements ; il n'y a pas de raison de les perdre.
///
/// Un UID qui n'existe plus fait échouer le rejeu d'une erreur définitive, que la
/// boucle traite déjà en abandonnant l'opération. Le pire cas est donc de revenir à
/// l'état actuel, ce qui est le bon pire cas.
///
/// La condition porte sur `'"to":'` plutôt que sur le type d'opération : elle décrit
/// exactement les lignes cassées, et une ligne saine que l'on toucherait par
/// approximation serait une régression introduite par une réparation.
const SCHEMA_V5: &str = r#"
UPDATE op_journal
SET payload = replace(payload, '"to":', '"target":'),
    done = 0,
    attempts = 0,
    next_attempt_at = 0,
    last_error = NULL
WHERE payload LIKE '%"op":"move"%' AND payload LIKE '%"to":%';
"#;

/// Takes out of the work queue every thread that only exists in the bin.
///
/// A mailbox triaged in another client — webmail, a phone — has most of its mail in
/// Trash already, and the first sync pulled all of it in as work to do. On the real
/// mailbox this was 485 messages of the 740: two thirds of the queue was rubbish
/// somebody had already thrown away.
///
/// Worse, Delete could not clear them. Moving a message that is already at its
/// destination produces nothing to send, the action reported "unchanged", and the
/// button appeared broken. That is fixed in the workflow; this is the backlog it
/// leaves behind, and pressing Delete four hundred times is not a fix.
///
/// Only threads whose messages are *all* in a bin or a junk folder. A conversation
/// with one message deleted and a reply still in the inbox is live work, and the one
/// deleted message says nothing about the other.
///
/// 2 is `WorkflowState::Done`, checked against the constant by a test below.
const SCHEMA_V4: &str = r#"
UPDATE threads
SET state = 2
WHERE state = 0
  AND EXISTS (SELECT 1 FROM messages WHERE messages.thread_id = threads.id)
  AND NOT EXISTS (
        SELECT 1
        FROM messages
        JOIN folders ON folders.id = messages.folder_id
        WHERE messages.thread_id = threads.id
          AND folders.role NOT IN ('trash', 'junk')
  );
"#;

/// Marks mail that was already in the database when spam detection arrived.
///
/// A flag only set on arrival would have left every message received before the
/// feature looking like ordinary mail for ever — which is exactly what a user sees
/// as "the spam filter does not work".
///
/// Only the subject marker is available here: the headers are not stored, and
/// re-fetching every message to read them would take hours. That is the weaker of
/// the two signals, but it is the one that put `***Potentiel-SPAM***` on screen, and
/// anything it misses gets caught on the next sync.
///
/// 512 is `Flags::SPAM`, checked against the constant by a test in this module.
const SCHEMA_V3: &str = r#"
UPDATE messages
SET flags = flags | 512
WHERE lower(subject) LIKE '%***spam***%'
   OR lower(subject) LIKE '%***potentiel-spam***%'
   OR lower(subject) LIKE '%***potential-spam***%'
   OR lower(subject) LIKE '%[spam]%'
   OR lower(subject) LIKE '%[spam?]%'
   OR lower(subject) LIKE '%{spam}%';

-- The list reads the thread, not the message, so the union needs the bit too.
-- Only ever setting it, never clearing, means this cannot disturb any other flag:
-- SQLite has no bitwise-or aggregate, and rebuilding the union with arithmetic
-- would quietly corrupt every thread it touched.
UPDATE threads
SET flags_union = flags_union | 512
WHERE EXISTS (
    SELECT 1 FROM messages
    WHERE messages.thread_id = threads.id AND (messages.flags & 512) != 0
);
"#;

/// Rules, and where they have already been applied.
///
/// The conditions and actions are stored as JSON rather than in columns. They are a
/// small tree with a shape that will grow — a new condition kind is a new variant, not
/// a new table — and nothing queries inside them: rules are read in bulk, evaluated in
/// memory, and there are tens of them, not millions.
///
/// `applied_to` is what stops a rule acting twice on the same message. Without it, a
/// rule that snoozes would push its own target forward on every synchronisation, and
/// the message would never come back.
const SCHEMA_V2: &str = r#"
CREATE TABLE rules (
    id          TEXT    PRIMARY KEY,
    name        TEXT    NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,
    position    INTEGER NOT NULL,
    definition  TEXT    NOT NULL
) STRICT;

CREATE INDEX rules_by_position ON rules(position);

CREATE TABLE rule_applications (
    rule_id    TEXT    NOT NULL,
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    applied_at INTEGER NOT NULL,
    PRIMARY KEY (rule_id, message_id)
) STRICT, WITHOUT ROWID;
"#;

/// Le schéma initial.
///
/// Trois choix méritent explication :
///
/// - **`threads` porte l'état de workflow**, pas `messages` : on traite un échange,
///   pas un message isolé.
/// - **`threads_by_state`** est l'index directeur de la liste principale. Sa forme
///   `(state, last_activity_at DESC, id DESC)` est exactement celle de la pagination
///   par curseur, ce qui permet à SQLite de servir une page sans jamais trier.
/// - **`messages.preview`** est calculé une fois à la synchronisation. Le
///   défilement ne doit jamais déclencher d'analyse MIME.
const SCHEMA_V1: &str = r#"
CREATE TABLE accounts (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    email             TEXT    NOT NULL UNIQUE,
    display_name      TEXT    NOT NULL DEFAULT '',
    imap_host         TEXT    NOT NULL,
    imap_port         INTEGER NOT NULL,
    imap_tls          INTEGER NOT NULL DEFAULT 1,
    smtp_host         TEXT    NOT NULL,
    smtp_port         INTEGER NOT NULL,
    smtp_tls          INTEGER NOT NULL DEFAULT 1,
    auth_kind         TEXT    NOT NULL DEFAULT 'password',
    group_name        TEXT,
    pinned            INTEGER NOT NULL DEFAULT 0,
    enabled           INTEGER NOT NULL DEFAULT 1,
    created_at        INTEGER NOT NULL,
    last_activity_at  INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE folders (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id      INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    path            TEXT    NOT NULL,
    role            TEXT    NOT NULL DEFAULT 'other',
    uid_validity    INTEGER NOT NULL DEFAULT 0,
    uid_next        INTEGER NOT NULL DEFAULT 0,
    highest_modseq  INTEGER NOT NULL DEFAULT 0,
    UNIQUE(account_id, path)
) STRICT;

CREATE TABLE threads (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    subject_norm      TEXT    NOT NULL DEFAULT '',
    state             INTEGER NOT NULL DEFAULT 0,
    snooze_until      INTEGER,
    snooze_restore    INTEGER,
    last_activity_at  INTEGER NOT NULL DEFAULT 0,
    message_count     INTEGER NOT NULL DEFAULT 0,
    unread_count      INTEGER NOT NULL DEFAULT 0,
    flags_union       INTEGER NOT NULL DEFAULT 0,
    -- Colonnes dénormalisées pour que la liste n'ait besoin d'aucune jointure.
    last_from_name    TEXT    NOT NULL DEFAULT '',
    last_from_addr    TEXT    NOT NULL DEFAULT '',
    last_subject      TEXT    NOT NULL DEFAULT '',
    last_preview      TEXT    NOT NULL DEFAULT ''
) STRICT;

CREATE INDEX threads_by_state
    ON threads(state, last_activity_at DESC, id DESC);
CREATE INDEX threads_snoozed
    ON threads(snooze_until) WHERE snooze_until IS NOT NULL;

CREATE TABLE messages (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id      INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    folder_id       INTEGER NOT NULL REFERENCES folders(id)  ON DELETE CASCADE,
    thread_id       INTEGER NOT NULL REFERENCES threads(id),
    uid             INTEGER NOT NULL,
    rfc_message_id  TEXT,
    in_reply_to     TEXT,
    subject         TEXT    NOT NULL DEFAULT '',
    from_name       TEXT    NOT NULL DEFAULT '',
    from_addr       TEXT    NOT NULL DEFAULT '',
    recipients      TEXT    NOT NULL DEFAULT '[]',
    date            INTEGER NOT NULL DEFAULT 0,
    received        INTEGER NOT NULL DEFAULT 0,
    size            INTEGER NOT NULL DEFAULT 0,
    flags           INTEGER NOT NULL DEFAULT 0,
    preview         TEXT    NOT NULL DEFAULT '',
    body_blob       TEXT,
    unsubscribe     TEXT,
    UNIQUE(folder_id, uid)
) STRICT;

CREATE INDEX messages_by_thread  ON messages(thread_id, received);
CREATE INDEX messages_by_account ON messages(account_id, received DESC);
CREATE INDEX messages_by_rfc_id  ON messages(rfc_message_id) WHERE rfc_message_id IS NOT NULL;
-- Sans cet index, le rattachement d'une reponse arrivee avant son original balaye
-- toute la table a chaque insertion, ce qui rend la synchronisation quadratique.
CREATE INDEX messages_by_in_reply_to ON messages(in_reply_to) WHERE in_reply_to IS NOT NULL;

-- Chaîne References, utilisée par le regroupement en fils.
CREATE TABLE message_refs (
    message_id  INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    ref_id      TEXT    NOT NULL,
    PRIMARY KEY(message_id, position)
) STRICT;

CREATE INDEX message_refs_by_ref ON message_refs(ref_id);

CREATE TABLE attachments (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id  INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    filename    TEXT    NOT NULL,
    mime_type   TEXT    NOT NULL,
    size        INTEGER NOT NULL,
    blob        TEXT,
    inline      INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE INDEX attachments_by_message ON attachments(message_id);

-- Quels comptes participent à un fil. Permet de filtrer une liste unifiée par
-- compte sans jointure sur messages.
CREATE TABLE thread_accounts (
    thread_id   INTEGER NOT NULL REFERENCES threads(id)   ON DELETE CASCADE,
    account_id  INTEGER NOT NULL REFERENCES accounts(id)  ON DELETE CASCADE,
    PRIMARY KEY(thread_id, account_id)
) STRICT;

CREATE INDEX thread_accounts_by_account ON thread_accounts(account_id);

-- La seule mémoire du système : à qui l'utilisateur a déjà écrit.
CREATE TABLE contacts_seen (
    addr_key        TEXT    PRIMARY KEY,
    display         TEXT    NOT NULL DEFAULT '',
    received_count  INTEGER NOT NULL DEFAULT 0,
    replied_count   INTEGER NOT NULL DEFAULT 0,
    last_seen_at    INTEGER NOT NULL DEFAULT 0
) STRICT;

-- Journal d'opérations : toute action locale en attente de réconciliation.
CREATE TABLE op_journal (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id       INTEGER NOT NULL,
    kind             TEXT    NOT NULL,
    payload          TEXT    NOT NULL,
    idempotency_key  TEXT    NOT NULL UNIQUE,
    created_at       INTEGER NOT NULL,
    attempts         INTEGER NOT NULL DEFAULT 0,
    next_attempt_at  INTEGER NOT NULL DEFAULT 0,
    last_error       TEXT,
    done             INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE INDEX op_journal_pending
    ON op_journal(done, next_attempt_at, id);

CREATE TABLE settings (
    key    TEXT PRIMARY KEY,
    value  TEXT NOT NULL
) STRICT;
"#;

/// Applique les migrations manquantes, dans une transaction par migration.
///
/// Retourne la version atteinte.
pub fn migrate(conn: &Connection) -> Result<i64> {
    let mut version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| Error::store(format!("lecture de la version : {e}")))?;

    if version > CURRENT_VERSION {
        return Err(Error::store(format!(
            "base écrite par une version plus récente d'Iris (schéma {version}, \
             cette version en connaît {CURRENT_VERSION})"
        )));
    }

    let pending: Vec<&Migration> = MIGRATIONS.iter().filter(|m| m.version > version).collect();
    for m in pending {
        tracing::info!(version = m.version, name = m.name, "migration");
        conn.execute_batch(&format!(
            "BEGIN; {} PRAGMA user_version = {}; COMMIT;",
            m.sql, m.version
        ))
        .map_err(|e| Error::store(format!("migration {} « {} » : {e}", m.version, m.name)))?;
        version = m.version;
    }

    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn une_base_neuve_atteint_la_version_courante() {
        let c = conn();
        assert_eq!(migrate(&c).unwrap(), CURRENT_VERSION);
    }

    #[test]
    fn migrer_deux_fois_ne_change_rien() {
        let c = conn();
        migrate(&c).unwrap();
        assert_eq!(migrate(&c).unwrap(), CURRENT_VERSION);
    }

    #[test]
    fn une_base_trop_recente_est_refusee_au_lieu_d_etre_corrompue() {
        let c = conn();
        c.execute_batch("PRAGMA user_version = 9999").unwrap();
        let e = migrate(&c).unwrap_err();
        assert!(e.to_string().contains("plus récente"));
    }

    #[test]
    fn les_versions_de_migration_sont_ordonnees_et_uniques() {
        let mut vus = std::collections::BTreeSet::new();
        let mut precedent = 0;
        for m in MIGRATIONS {
            assert!(m.version > precedent, "migrations non ordonnées");
            assert!(vus.insert(m.version), "version dupliquée");
            precedent = m.version;
        }
        assert_eq!(precedent, CURRENT_VERSION);
    }

    #[test]
    fn l_index_directeur_de_la_liste_existe() {
        let c = conn();
        migrate(&c).unwrap();
        let n: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='index' AND name='threads_by_state'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn the_spam_bit_in_sql_matches_the_constant() {
        // The migration interpolates 512 into query text; if the flag ever moves,
        // this catches it before the wrong bit is set on someone's mailbox.
        assert_eq!(iris_types::Flags::SPAM.0, 512);
    }

    #[test]
    fn the_backfill_marks_mail_that_arrived_before_the_feature() {
        // A flag only set on arrival leaves everything already downloaded looking
        // like ordinary mail for ever, which reads as "the spam filter is broken".
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();

        conn.execute_batch(
            "INSERT INTO accounts (id, email, imap_host, imap_port, smtp_host, smtp_port, created_at)
             VALUES (1, 'a@x.fr', 'i', 993, 's', 465, 0);
             INSERT INTO folders (id, account_id, path, role) VALUES (1, 1, 'INBOX', 'inbox');
             INSERT INTO threads (id, subject_norm, state, last_activity_at, flags_union)
             VALUES (1, 'a', 0, 0, 0), (2, 'b', 0, 0, 0);
             INSERT INTO messages
                (id, account_id, folder_id, thread_id, uid, subject, from_name, from_addr,
                 recipients, date, received, size, flags, preview)
             VALUES
                (1, 1, 1, 1, 1, '***Potentiel-SPAM*** Virement', 'x', 'x@y.fr', '[]', 0, 0, 1, 0, ''),
                (2, 1, 1, 2, 2, 'Quote for the cylinders', 'x', 'x@y.fr', '[]', 0, 0, 1, 0, '');",
        )
        .unwrap();

        conn.execute_batch(SCHEMA_V3).unwrap();

        let spam: i64 = conn
            .query_row("SELECT flags FROM messages WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        let ordinary: i64 = conn
            .query_row("SELECT flags FROM messages WHERE id = 2", [], |r| r.get(0))
            .unwrap();

        assert_eq!(spam & 512, 512, "the tagged subject must be marked");
        assert_eq!(ordinary & 512, 0, "ordinary mail must be left alone");

        // And the thread, because the list reads the thread rather than the message.
        let thread: i64 = conn
            .query_row("SELECT flags_union FROM threads WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(thread & 512, 512);
    }

    #[test]
    fn les_deplacements_casses_sont_repares_et_remis_en_attente() {
        // Ce sont des actions que l'utilisateur a demandées et qui n'ont jamais eu
        // lieu. Corriger le code sans les réparer laisserait le courrier là où il
        // n'aurait jamais dû rester.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        for sql in [SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4] {
            conn.execute_batch(sql).unwrap();
        }

        conn.execute_batch(
            r#"INSERT INTO op_journal
                 (id, account_id, kind, payload, idempotency_key, created_at, done, attempts)
               VALUES
                 (1, 1, 'move_message',
                  '{"op":"move","folder":"INBOX","uids":[7],"to":"INBOX.Archive"}',
                  'k1', 0, 1, 3),
                 (2, 1, 'set_flags',
                  '{"op":"set_flags","folder":"INBOX","uids":[8],"flags":4,"add":true}',
                  'k2', 0, 1, 0);"#,
        )
        .unwrap();

        conn.execute_batch(SCHEMA_V5).unwrap();

        let (charge, fini, essais): (String, i64, i64) = conn
            .query_row(
                "SELECT payload, done, attempts FROM op_journal WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();

        assert!(
            charge.contains(r#""target":"INBOX.Archive""#),
            "obtenu : {charge}"
        );
        assert!(
            !charge.contains(r#""to":"#),
            "l'ancien nom doit disparaître"
        );
        assert_eq!(fini, 0, "l'opération doit être rejouée");
        assert_eq!(essais, 0, "et repartir d'un compteur neuf");
    }

    #[test]
    fn les_operations_saines_ne_sont_pas_touchees() {
        // Une ligne saine réveillée par une réparation serait une régression
        // introduite par la réparation elle-même.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        for sql in [SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4] {
            conn.execute_batch(sql).unwrap();
        }

        conn.execute_batch(
            r#"INSERT INTO op_journal
                 (id, account_id, kind, payload, idempotency_key, created_at, done)
               VALUES
                 (1, 1, 'set_flags',
                  '{"op":"set_flags","folder":"INBOX","uids":[8],"flags":4,"add":true}',
                  'k1', 0, 1),
                 (2, 1, 'move_message',
                  '{"op":"move","folder":"INBOX","uids":[9],"target":"INBOX.Archive"}',
                  'k2', 0, 1);"#,
        )
        .unwrap();

        conn.execute_batch(SCHEMA_V5).unwrap();

        let reveilles: i64 = conn
            .query_row("SELECT count(*) FROM op_journal WHERE done = 0", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            reveilles, 0,
            "aucune ligne saine ne doit être remise en file"
        );
    }

    #[test]
    fn deux_est_bien_l_etat_termine() {
        // La migration écrit un nombre en dur : si l'encodage change, elle doit
        // échouer ici plutôt que de ranger silencieusement les fils dans la mauvaise
        // file de tous les utilisateurs.
        assert_eq!(iris_types::WorkflowState::Done.as_i64(), 2);
        assert_eq!(iris_types::WorkflowState::Todo.as_i64(), 0);
    }

    #[test]
    fn un_fil_entierement_a_la_corbeille_quitte_la_file() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();
        conn.execute_batch(SCHEMA_V3).unwrap();

        conn.execute_batch(
            "INSERT INTO accounts (id, email, imap_host, imap_port, smtp_host, smtp_port, created_at)
             VALUES (1, 'a@x.fr', 'i', 993, 's', 465, 0);
             INSERT INTO folders (id, account_id, path, role) VALUES (1, 1, 'INBOX', 'inbox');
             INSERT INTO folders (id, account_id, path, role) VALUES (2, 1, 'Trash', 'trash');
             INSERT INTO threads (id, subject_norm, state, last_activity_at, flags_union)
             VALUES (1, 'jete', 0, 0, 0), (2, 'vivant', 0, 0, 0), (3, 'mixte', 0, 0, 0);
             INSERT INTO messages
                (id, account_id, folder_id, thread_id, uid, subject, from_name, from_addr,
                 recipients, date, received, size, flags, preview)
             VALUES (1, 1, 2, 1, 1, 'jete', 'x', 'x@y.fr', '[]', 0, 0, 1, 0, ''),
                    (2, 1, 1, 2, 2, 'vivant', 'x', 'x@y.fr', '[]', 0, 0, 1, 0, ''),
                    (3, 1, 2, 3, 3, 'mixte a', 'x', 'x@y.fr', '[]', 0, 0, 1, 0, ''),
                    (4, 1, 1, 3, 4, 'mixte b', 'x', 'x@y.fr', '[]', 0, 0, 1, 0, '');",
        )
        .unwrap();

        conn.execute_batch(SCHEMA_V4).unwrap();

        let etat = |id: i64| -> i64 {
            conn.query_row("SELECT state FROM threads WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
        };

        assert_eq!(etat(1), 2, "un fil entièrement jeté sort de la file");
        assert_eq!(
            etat(2),
            0,
            "un fil de la boîte de réception reste à traiter"
        );
        assert_eq!(
            etat(3),
            0,
            "un message jeté ne dit rien de la réponse restée dans la boîte"
        );
    }

    #[test]
    fn un_fil_deja_classe_n_est_pas_deplace() {
        // Seule la file « à traiter » est concernée. Un fil rangé en attente par
        // l'utilisateur est une décision qui lui appartient.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();
        conn.execute_batch(SCHEMA_V3).unwrap();

        conn.execute_batch(
            "INSERT INTO accounts (id, email, imap_host, imap_port, smtp_host, smtp_port, created_at)
             VALUES (1, 'a@x.fr', 'i', 993, 's', 465, 0);
             INSERT INTO folders (id, account_id, path, role) VALUES (2, 1, 'Trash', 'trash');
             INSERT INTO threads (id, subject_norm, state, last_activity_at, flags_union)
             VALUES (1, 'attente', 1, 0, 0);
             INSERT INTO messages
                (id, account_id, folder_id, thread_id, uid, subject, from_name, from_addr,
                 recipients, date, received, size, flags, preview)
             VALUES (1, 1, 2, 1, 1, 'attente', 'x', 'x@y.fr', '[]', 0, 0, 1, 0, '');",
        )
        .unwrap();

        conn.execute_batch(SCHEMA_V4).unwrap();

        let etat: i64 = conn
            .query_row("SELECT state FROM threads WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(etat, 1);
    }

    #[test]
    fn the_backfill_leaves_other_flags_untouched() {
        // SQLite has no bitwise-or aggregate; rebuilding the union with arithmetic
        // would quietly corrupt every thread it touched.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();

        conn.execute_batch(
            "INSERT INTO accounts (id, email, imap_host, imap_port, smtp_host, smtp_port, created_at)
             VALUES (1, 'a@x.fr', 'i', 993, 's', 465, 0);
             INSERT INTO folders (id, account_id, path, role) VALUES (1, 1, 'INBOX', 'inbox');
             INSERT INTO threads (id, subject_norm, state, last_activity_at, flags_union)
             VALUES (1, 'a', 0, 0, 5);
             INSERT INTO messages
                (id, account_id, folder_id, thread_id, uid, subject, from_name, from_addr,
                 recipients, date, received, size, flags, preview)
             VALUES (1, 1, 1, 1, 1, '[SPAM] hello', 'x', 'x@y.fr', '[]', 0, 0, 1, 5, '');",
        )
        .unwrap();

        conn.execute_batch(SCHEMA_V3).unwrap();

        let flags: i64 = conn
            .query_row("SELECT flags FROM messages WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(flags, 5 | 512, "seen and flagged must survive");

        let union: i64 = conn
            .query_row("SELECT flags_union FROM threads WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(union, 5 | 512);
    }
}
