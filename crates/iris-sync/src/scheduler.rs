//! L'ordonnanceur.
//!
//! Cent comptes, seize connexions : il faut choisir en permanence qui parle au
//! réseau. Deux mécanismes s'en chargent.
//!
//! **La priorité** décide de l'ordre : le compte que l'utilisateur regarde passe
//! avant ses comptes épinglés, qui passent avant ceux qui ont bougé récemment, qui
//! passent avant le reste. Sans cet ordre, la boîte consultée attendrait derrière
//! quatre-vingt-dix-neuf autres.
//!
//! **L'intervalle adaptatif** décide de la fréquence : une boîte qui reçoit dix
//! messages par heure est interrogée souvent, une boîte muette depuis six mois est
//! interrogée rarement. L'intervalle se resserre à chaque nouveauté et s'élargit à
//! chaque tour à vide, ce qui fait converger le coût total vers l'activité réelle
//! plutôt que vers le nombre de comptes.

use iris_types::{AccountId, Timestamp};
use std::collections::BTreeMap;
use std::time::Duration;

/// Classe de priorité d'un compte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// Le compte affiché à l'écran.
    Active,
    /// Épinglé par l'utilisateur.
    Pinned,
    /// A reçu du courrier récemment.
    Recent,
    /// Tout le reste.
    Background,
}

impl Priority {
    /// Ce compte mérite-t-il une connexion permanente ?
    ///
    /// Seuls les deux premiers rangs : au-delà, le sondage périodique suffit et
    /// coûte infiniment moins cher.
    pub fn deserves_idle(self) -> bool {
        matches!(self, Self::Active | Self::Pinned)
    }
}

/// Réglages de l'ordonnancement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleConfig {
    pub min_interval: Duration,
    pub max_interval: Duration,
    /// Intervalle d'un compte dont on ne sait encore rien.
    pub initial_interval: Duration,
    /// Un compte reste « récent » pendant cette durée après une nouveauté.
    pub recent_window: Duration,
    /// Nombre d'échecs consécutifs au-delà duquel le compte est mis de côté.
    pub failure_threshold: u32,
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            min_interval: Duration::from_secs(60),
            // A quiet mailbox is still looked at every quarter of an hour: at an
            // hour, new mail could take that long to show.
            max_interval: Duration::from_secs(900),
            initial_interval: Duration::from_secs(300),
            recent_window: Duration::from_secs(6 * 3600),
            failure_threshold: 5,
        }
    }
}

/// Ce qu'un cycle de synchronisation a rapporté.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOutcome {
    /// Des messages sont arrivés ou ont changé.
    Changed { messages: u32 },
    /// Rien de neuf.
    Unchanged,
    /// Échec passager : le compte sera revu, mais plus tard.
    TransientFailure,
    /// Échec exigeant une intervention : le compte est suspendu.
    NeedsAttention,
}

/// L'état d'ordonnancement d'un compte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSchedule {
    pub account: AccountId,
    pub server: String,
    pub pinned: bool,
    pub priority: Priority,
    pub interval: Duration,
    pub next_due: Timestamp,
    pub last_activity: Timestamp,
    pub consecutive_failures: u32,
    /// In trouble: shown to the user. Tried again on its own after a while unless
    /// `needs_user`.
    pub suspended: bool,
    /// Stopped until the user acts: a refused password does not fix itself.
    pub needs_user: bool,
}

impl AccountSchedule {
    pub fn is_due(&self, now: Timestamp) -> bool {
        !self.needs_user && now.millis() >= self.next_due.millis()
    }
}

/// L'ordonnanceur.
#[derive(Debug)]
pub struct Scheduler {
    config: ScheduleConfig,
    accounts: BTreeMap<AccountId, AccountSchedule>,
    active: Option<AccountId>,
}

impl Scheduler {
    pub fn new(config: ScheduleConfig) -> Self {
        Self {
            config,
            accounts: BTreeMap::new(),
            active: None,
        }
    }

    pub fn config(&self) -> ScheduleConfig {
        self.config
    }

    /// Inscrit un compte, ou met à jour ses attributs.
    pub fn register(&mut self, account: AccountId, server: &str, pinned: bool, now: Timestamp) {
        let config = self.config;
        let entree = self
            .accounts
            .entry(account)
            .or_insert_with(|| AccountSchedule {
                account,
                server: server.to_string(),
                pinned,
                priority: Priority::Background,
                interval: config.initial_interval,
                // Un compte fraîchement inscrit est immédiatement dû : il faut bien le
                // synchroniser une première fois.
                next_due: now,
                last_activity: Timestamp::EPOCH,
                consecutive_failures: 0,
                suspended: false,
                needs_user: false,
            });
        entree.server = server.to_string();
        entree.pinned = pinned;
        self.recompute_priority(account, now);
    }

    pub fn remove(&mut self, account: AccountId) {
        self.accounts.remove(&account);
        if self.active == Some(account) {
            self.active = None;
        }
    }

    /// Désigne le compte que l'utilisateur regarde.
    pub fn set_active(&mut self, account: Option<AccountId>, now: Timestamp) {
        let precedent = self.active;
        self.active = account;

        for id in [precedent, account].into_iter().flatten() {
            self.recompute_priority(id, now);
        }

        // Le compte qui vient d'être ouvert doit être rafraîchi tout de suite :
        // l'utilisateur le regarde, il ne peut pas attendre le prochain cycle.
        if let Some(id) = account {
            if let Some(e) = self.accounts.get_mut(&id) {
                if !e.needs_user {
                    e.next_due = now;
                }
            }
        }
    }

    fn recompute_priority(&mut self, account: AccountId, now: Timestamp) {
        let active = self.active;
        let fenetre = self.config.recent_window.as_millis() as i64;
        if let Some(e) = self.accounts.get_mut(&account) {
            e.priority = if active == Some(account) {
                Priority::Active
            } else if e.pinned {
                Priority::Pinned
            } else if e.last_activity.millis() + fenetre >= now.millis() {
                Priority::Recent
            } else {
                Priority::Background
            };
        }
    }

    /// Les comptes à synchroniser maintenant, du plus prioritaire au moins.
    ///
    /// À priorité égale, le plus en retard passe devant : sans ce départage, un
    /// compte pourrait rester indéfiniment derrière ses pairs.
    pub fn due(&mut self, now: Timestamp) -> Vec<AccountId> {
        let ids: Vec<AccountId> = self.accounts.keys().copied().collect();
        for id in ids {
            self.recompute_priority(id, now);
        }

        let mut dus: Vec<&AccountSchedule> =
            self.accounts.values().filter(|e| e.is_due(now)).collect();

        dus.sort_by_key(|e| (e.priority, e.next_due.millis(), e.account.get()));
        dus.into_iter().map(|e| e.account).collect()
    }

    /// Les comptes méritant une connexion permanente, par ordre de priorité.
    pub fn idle_candidates(&self, capacity: usize) -> Vec<AccountId> {
        let mut candidats: Vec<&AccountSchedule> = self
            .accounts
            .values()
            .filter(|e| !e.suspended && e.priority.deserves_idle())
            .collect();
        candidats.sort_by_key(|e| (e.priority, e.account.get()));
        candidats
            .into_iter()
            .take(capacity)
            .map(|e| e.account)
            .collect()
    }

    /// Enregistre le résultat d'un cycle et ajuste l'intervalle.
    pub fn record(&mut self, account: AccountId, outcome: SyncOutcome, now: Timestamp) {
        let config = self.config;
        let Some(e) = self.accounts.get_mut(&account) else {
            return;
        };

        match outcome {
            SyncOutcome::Changed { .. } => {
                e.consecutive_failures = 0;
                e.suspended = false;
                e.needs_user = false;
                e.last_activity = now;
                // Retour immédiat à l'intervalle minimal : une boîte qui vient de
                // recevoir est susceptible de recevoir encore.
                e.interval = config.min_interval;
            }
            SyncOutcome::Unchanged => {
                e.consecutive_failures = 0;
                e.suspended = false;
                e.needs_user = false;
                // Élargissement progressif. Le facteur trois quarts fait converger
                // vers l'intervalle maximal en une dizaine de tours à vide, ce qui
                // laisse le temps de réagir à une reprise d'activité.
                let elargi = e.interval.saturating_add(e.interval / 2);
                // The mailbox on screen stays close: someone is looking at it.
                let plafond = if e.priority == Priority::Active {
                    (config.min_interval * 2).min(config.max_interval)
                } else {
                    config.max_interval
                };
                e.interval = elargi.min(plafond);
            }
            SyncOutcome::TransientFailure => {
                e.consecutive_failures += 1;
                // Repli exponentiel, plafonné : un serveur en panne ne doit pas être
                // martelé, mais doit rester surveillé.
                let facteur = 2u32.saturating_pow(e.consecutive_failures.min(6));
                e.interval = (config.min_interval * facteur).min(config.max_interval);
                // Marked as in trouble, for the user to see, but tried again on its
                // own at the longest interval: a network that comes back, a server out
                // for half an hour, a long first sync that ran out of time are not the
                // user's to fix. Suspended for good, an account stopped after about
                // thirty minutes offline until someone clicked it.
                if e.consecutive_failures >= config.failure_threshold {
                    e.suspended = true;
                    e.interval = config.max_interval;
                }
            }
            SyncOutcome::NeedsAttention => {
                // Un mot de passe faux ne se corrige pas tout seul.
                e.consecutive_failures += 1;
                e.suspended = true;
                e.needs_user = true;
            }
        }

        e.next_due = Timestamp::from_millis(now.millis() + e.interval.as_millis() as i64);
    }

    /// Réactive un compte suspendu, après correction par l'utilisateur.
    pub fn resume(&mut self, account: AccountId, now: Timestamp) {
        if let Some(e) = self.accounts.get_mut(&account) {
            e.suspended = false;
            e.needs_user = false;
            e.consecutive_failures = 0;
            e.interval = self.config.min_interval;
            e.next_due = now;
        }
    }

    /// The accounts registered.
    pub fn registered(&self) -> Vec<AccountId> {
        self.accounts.keys().copied().collect()
    }

    pub fn get(&self, account: AccountId) -> Option<&AccountSchedule> {
        self.accounts.get(&account)
    }

    pub fn len(&self) -> usize {
        self.accounts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    pub fn suspended(&self) -> Vec<AccountId> {
        self.accounts
            .values()
            .filter(|e| e.suspended)
            .map(|e| e.account)
            .collect()
    }

    /// Instant du prochain réveil nécessaire.
    ///
    /// Permet à la boucle de synchronisation de dormir exactement le temps qu'il
    /// faut au lieu de se réveiller toutes les secondes pour ne rien faire.
    pub fn next_wakeup(&self, now: Timestamp) -> Option<Duration> {
        self.accounts
            .values()
            .filter(|e| !e.needs_user)
            .map(|e| e.next_due.millis().max(now.millis()) - now.millis())
            .min()
            .map(|ms| Duration::from_millis(ms as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(secs: i64) -> Timestamp {
        Timestamp::from_millis(secs * 1000)
    }

    fn scheduler() -> Scheduler {
        Scheduler::new(ScheduleConfig::default())
    }

    #[test]
    fn un_compte_neuf_est_immediatement_du() {
        let mut s = scheduler();
        s.register(AccountId(1), "imap.x.fr", false, t(0));
        assert_eq!(s.due(t(0)), [AccountId(1)]);
    }

    #[test]
    fn le_compte_regarde_passe_devant_tous_les_autres() {
        let mut s = scheduler();
        for i in 1..=5 {
            s.register(AccountId(i), "imap.x.fr", false, t(0));
        }
        s.set_active(Some(AccountId(4)), t(0));

        assert_eq!(s.due(t(0))[0], AccountId(4));
    }

    #[test]
    fn les_epingles_passent_avant_le_reste() {
        let mut s = scheduler();
        s.register(AccountId(1), "x", false, t(0));
        s.register(AccountId(2), "x", true, t(0));
        s.register(AccountId(3), "x", false, t(0));

        assert_eq!(s.due(t(0))[0], AccountId(2));
    }

    #[test]
    fn ouvrir_un_compte_le_rend_immediatement_du() {
        let mut s = scheduler();
        s.register(AccountId(1), "x", false, t(0));
        s.record(AccountId(1), SyncOutcome::Unchanged, t(0));
        assert!(s.due(t(10)).is_empty(), "l'intervalle n'est pas écoulé");

        s.set_active(Some(AccountId(1)), t(10));
        assert_eq!(s.due(t(10)), [AccountId(1)], "l'utilisateur le regarde");
    }

    #[test]
    fn l_intervalle_se_resserre_des_qu_il_y_a_du_nouveau() {
        let mut s = scheduler();
        s.register(AccountId(1), "x", false, t(0));

        // Plusieurs tours à vide élargissent l'intervalle.
        for i in 0..5 {
            s.record(AccountId(1), SyncOutcome::Unchanged, t(i * 1000));
        }
        let elargi = s.get(AccountId(1)).unwrap().interval;
        assert!(elargi > ScheduleConfig::default().initial_interval);

        // Une nouveauté le ramène au minimum.
        s.record(AccountId(1), SyncOutcome::Changed { messages: 3 }, t(6000));
        assert_eq!(
            s.get(AccountId(1)).unwrap().interval,
            ScheduleConfig::default().min_interval
        );
    }

    #[test]
    fn l_intervalle_ne_depasse_jamais_le_plafond() {
        let mut s = scheduler();
        s.register(AccountId(1), "x", false, t(0));
        for i in 0..100 {
            s.record(AccountId(1), SyncOutcome::Unchanged, t(i * 10_000));
        }
        assert_eq!(
            s.get(AccountId(1)).unwrap().interval,
            ScheduleConfig::default().max_interval
        );
    }

    #[test]
    fn un_compte_actif_devient_recent_puis_retombe_en_fond() {
        let mut s = scheduler();
        s.register(AccountId(1), "x", false, t(0));
        s.record(AccountId(1), SyncOutcome::Changed { messages: 1 }, t(0));

        s.due(t(100));
        assert_eq!(s.get(AccountId(1)).unwrap().priority, Priority::Recent);

        // Au-delà de la fenêtre, il redevient un compte ordinaire.
        let apres = t(ScheduleConfig::default().recent_window.as_secs() as i64 + 10);
        s.due(apres);
        assert_eq!(s.get(AccountId(1)).unwrap().priority, Priority::Background);
    }

    #[test]
    fn un_echec_passager_repousse_puis_suspend() {
        let mut s = scheduler();
        s.register(AccountId(1), "x", false, t(0));

        for i in 1..ScheduleConfig::default().failure_threshold {
            s.record(
                AccountId(1),
                SyncOutcome::TransientFailure,
                t(i as i64 * 100),
            );
            assert!(!s.get(AccountId(1)).unwrap().suspended, "pas encore");
        }

        s.record(AccountId(1), SyncOutcome::TransientFailure, t(10_000));
        assert!(s.get(AccountId(1)).unwrap().suspended);
        assert_eq!(s.suspended(), [AccountId(1)]);
        let pause = ScheduleConfig::default().max_interval.as_secs() as i64;
        assert!(
            s.due(t(10_000 + pause - 1)).is_empty(),
            "left alone for a while"
        );
        // Then tried again on its own: a network back or a server up again is not
        // the user's to fix. It stayed stopped until someone clicked it.
        assert_eq!(s.due(t(10_000 + pause)), [AccountId(1)]);
        s.record(AccountId(1), SyncOutcome::Unchanged, t(10_000 + pause));
        assert!(
            s.suspended().is_empty(),
            "working again, no longer in trouble"
        );
    }

    #[test]
    fn a_refused_password_stays_stopped() {
        let mut s = scheduler();
        s.register(AccountId(1), "x", false, t(0));
        s.record(AccountId(1), SyncOutcome::NeedsAttention, t(0));
        assert!(s.due(t(999_999)).is_empty());
    }

    #[test]
    fn le_repli_apres_echec_est_exponentiel() {
        let mut s = scheduler();
        s.register(AccountId(1), "x", false, t(0));

        s.record(AccountId(1), SyncOutcome::TransientFailure, t(0));
        let premier = s.get(AccountId(1)).unwrap().interval;
        s.record(AccountId(1), SyncOutcome::TransientFailure, t(1));
        let second = s.get(AccountId(1)).unwrap().interval;

        assert!(second > premier, "{second:?} devrait dépasser {premier:?}");
    }

    #[test]
    fn un_mot_de_passe_faux_suspend_immediatement() {
        // Le retenter cent fois ne le corrigera pas, et risque de bloquer le compte.
        let mut s = scheduler();
        s.register(AccountId(1), "x", false, t(0));
        s.record(AccountId(1), SyncOutcome::NeedsAttention, t(0));

        assert!(s.get(AccountId(1)).unwrap().suspended);
    }

    #[test]
    fn la_reprise_remet_le_compte_en_service() {
        let mut s = scheduler();
        s.register(AccountId(1), "x", false, t(0));
        s.record(AccountId(1), SyncOutcome::NeedsAttention, t(0));

        s.resume(AccountId(1), t(100));
        assert!(!s.get(AccountId(1)).unwrap().suspended);
        assert_eq!(s.due(t(100)), [AccountId(1)]);
    }

    #[test]
    fn seuls_les_comptes_prioritaires_recoivent_une_connexion_permanente() {
        let mut s = scheduler();
        for i in 1..=20 {
            s.register(AccountId(i), "x", i <= 3, t(0));
        }
        s.set_active(Some(AccountId(10)), t(0));
        s.due(t(0));

        let candidats = s.idle_candidates(16);
        assert_eq!(candidats.len(), 4, "un actif et trois épinglés");
        assert_eq!(candidats[0], AccountId(10), "l'actif d'abord");
    }

    #[test]
    fn la_capacite_borne_les_connexions_permanentes() {
        let mut s = scheduler();
        for i in 1..=50 {
            s.register(AccountId(i), "x", true, t(0));
        }
        s.due(t(0));
        assert_eq!(s.idle_candidates(16).len(), 16);
    }

    #[test]
    fn un_compte_suspendu_ne_recoit_pas_de_connexion_permanente() {
        let mut s = scheduler();
        s.register(AccountId(1), "x", true, t(0));
        s.record(AccountId(1), SyncOutcome::NeedsAttention, t(0));
        s.due(t(0));
        assert!(s.idle_candidates(16).is_empty());
    }

    #[test]
    fn a_priorite_egale_le_plus_en_retard_passe_devant() {
        // Sans ce départage, un compte pourrait rester indéfiniment derrière.
        let mut s = scheduler();
        s.register(AccountId(1), "x", false, t(0));
        s.register(AccountId(2), "x", false, t(0));

        s.record(AccountId(1), SyncOutcome::Unchanged, t(0));
        s.record(AccountId(2), SyncOutcome::Unchanged, t(200));

        let dus = s.due(t(100_000));
        assert_eq!(dus, [AccountId(1), AccountId(2)]);
    }

    #[test]
    fn le_prochain_reveil_est_calcule() {
        // La boucle doit dormir exactement le temps utile, pas se réveiller
        // inutilement chaque seconde.
        let mut s = scheduler();
        s.register(AccountId(1), "x", false, t(0));
        s.record(AccountId(1), SyncOutcome::Unchanged, t(0));

        let attente = s.next_wakeup(t(0)).unwrap();
        assert_eq!(
            attente,
            ScheduleConfig::default().initial_interval
                + ScheduleConfig::default().initial_interval / 2
        );

        // Un compte déjà dû ne fait pas attendre.
        s.register(AccountId(2), "x", false, t(0));
        assert_eq!(s.next_wakeup(t(0)), Some(Duration::ZERO));
    }

    #[test]
    fn sans_compte_il_n_y_a_pas_de_reveil() {
        assert!(scheduler().next_wakeup(t(0)).is_none());
    }

    #[test]
    fn retirer_un_compte_le_sort_de_l_ordonnancement() {
        let mut s = scheduler();
        s.register(AccountId(1), "x", false, t(0));
        s.set_active(Some(AccountId(1)), t(0));

        s.remove(AccountId(1));
        assert!(s.is_empty());
        assert!(s.due(t(0)).is_empty());
    }

    #[test]
    fn reinscrire_un_compte_met_a_jour_ses_attributs_sans_le_reinitialiser() {
        let mut s = scheduler();
        s.register(AccountId(1), "ancien.fr", false, t(0));
        s.record(AccountId(1), SyncOutcome::Unchanged, t(0));
        let intervalle = s.get(AccountId(1)).unwrap().interval;

        s.register(AccountId(1), "nouveau.fr", true, t(10));
        let e = s.get(AccountId(1)).unwrap();
        assert_eq!(e.server, "nouveau.fr");
        assert!(e.pinned);
        assert_eq!(
            e.interval, intervalle,
            "l'apprentissage ne doit pas être perdu"
        );
    }
}
