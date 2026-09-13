//! `iris-oauth` — connexion aux comptes Google et Microsoft.
//!
//! Ces deux fournisseurs refusent désormais les mots de passe : sans ce module, deux
//! des plus grosses boîtes du monde sont inaccessibles. Le flux retenu est le code
//! d'autorisation avec PKCE et redirection vers la boucle locale, seul flux
//! acceptable pour une application de bureau — un secret client embarqué dans un
//! binaire distribué n'est pas un secret.
//!
//! Ce qui se teste sans réseau est testé : construction de l'URL, vérificateur PKCE,
//! analyse des réponses, calcul de péremption. L'échange proprement dit passe par un
//! trait.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

use async_trait::async_trait;
use iris_types::{Error, Result, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Un fournisseur d'identité.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Google,
    Microsoft,
}

impl Provider {
    pub fn authorize_url(self) -> &'static str {
        match self {
            Self::Google => "https://accounts.google.com/o/oauth2/v2/auth",
            Self::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
        }
    }

    pub fn token_url(self) -> &'static str {
        match self {
            Self::Google => "https://oauth2.googleapis.com/token",
            Self::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/token",
        }
    }

    /// Habilitations demandées.
    ///
    /// Strictement le nécessaire : lire et envoyer du courrier, plus l'adresse du
    /// compte pour l'afficher. Demander davantage ferait hésiter l'utilisateur au
    /// moment le plus délicat, celui de l'autorisation.
    pub fn scopes(self) -> &'static [&'static str] {
        match self {
            Self::Google => &["https://mail.google.com/", "email"],
            Self::Microsoft => &[
                "https://outlook.office.com/IMAP.AccessAsUser.All",
                "https://outlook.office.com/SMTP.Send",
                "offline_access",
                "email",
            ],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Google => "Google",
            Self::Microsoft => "Microsoft",
        }
    }
}

/// Ce qu'un fournisseur nous a délivré.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    /// Absent lors d'un rafraîchissement : le fournisseur ne le renvoie pas toujours,
    /// et l'ancien reste alors valable.
    pub refresh_token: Option<String>,
    pub expires_at: Timestamp,
    /// Adresse du compte, quand le fournisseur la communique.
    pub email: Option<String>,
}

impl Tokens {
    /// Le jeton est-il périmé, ou sur le point de l'être ?
    ///
    /// La marge évite le cas classique où le jeton expire entre la vérification et
    /// l'usage, ce qui se produit d'autant plus qu'une synchronisation de cent
    /// comptes prend du temps.
    pub fn is_expired(&self, now: Timestamp, margin_secs: i64) -> bool {
        now.seconds() + margin_secs >= self.expires_at.seconds()
    }

    pub fn expires_in_secs(&self, now: Timestamp) -> i64 {
        self.expires_at.seconds() - now.seconds()
    }
}

/// Réponse brute du point d'accès aux jetons.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    /// Durée de validité, en secondes.
    expires_in: Option<i64>,
    #[allow(dead_code)]
    token_type: Option<String>,
    id_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Une demande d'autorisation en cours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRequest {
    pub provider: Provider,
    pub url: String,
    /// Vérificateur PKCE, à présenter lors de l'échange.
    pub verifier: String,
    /// Jeton anti-rejeu, à comparer au retour.
    pub state: String,
    pub redirect_uri: String,
}

/// Prépare une demande d'autorisation.
pub fn begin(
    provider: Provider,
    client_id: &str,
    redirect_port: u16,
    login_hint: Option<&str>,
    entropy: &[u8],
) -> AuthRequest {
    let verifier = pkce_verifier(entropy);
    let challenge = pkce_challenge(&verifier);
    let state = state_token(entropy);
    let redirect_uri = format!("http://127.0.0.1:{redirect_port}/oauth");

    let mut params: Vec<(&str, String)> = vec![
        ("client_id", client_id.to_string()),
        ("redirect_uri", redirect_uri.clone()),
        ("response_type", "code".into()),
        ("scope", provider.scopes().join(" ")),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256".into()),
        ("state", state.clone()),
    ];

    if provider == Provider::Google {
        // Sans ces deux paramètres, Google ne renvoie pas de jeton de
        // rafraîchissement à la deuxième autorisation, et le compte se déconnecte
        // au bout d'une heure sans explication.
        params.push(("access_type", "offline".into()));
        params.push(("prompt", "consent".into()));
    }

    if let Some(hint) = login_hint {
        params.push(("login_hint", hint.to_string()));
    }

    let requete = params
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");

    AuthRequest {
        provider,
        url: format!("{}?{requete}", provider.authorize_url()),
        verifier,
        state,
        redirect_uri,
    }
}

/// Analyse l'URL de retour de la redirection locale.
///
/// Vérifie le jeton anti-rejeu : sans cette comparaison, un site tiers pourrait
/// faire aboutir sa propre autorisation dans notre application.
pub fn parse_redirect(url: &str, expected_state: &str) -> Result<String> {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or(url);
    let params: BTreeMap<&str, String> = query
        .split('&')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k, urldecode(v)))
        .collect();

    if let Some(erreur) = params.get("error") {
        let details = params.get("error_description").cloned().unwrap_or_default();
        return Err(Error::Config(format!("autorisation refusée : {erreur} {details}")));
    }

    let state = params
        .get("state")
        .ok_or_else(|| Error::Config("réponse d'autorisation sans jeton anti-rejeu".into()))?;
    if state != expected_state {
        return Err(Error::Config(
            "jeton anti-rejeu incorrect : la réponse ne provient pas de cette demande".into(),
        ));
    }

    params
        .get("code")
        .cloned()
        .ok_or_else(|| Error::Config("réponse d'autorisation sans code".into()))
}

/// Ce qui sait dialoguer avec le point d'accès aux jetons.
#[async_trait]
pub trait TokenEndpoint: Send + Sync + std::fmt::Debug {
    /// Envoie une requête encodée en formulaire et rend le corps de la réponse.
    async fn post_form(&self, url: &str, params: &[(String, String)]) -> Result<String>;
}

/// Échange un code d'autorisation contre des jetons.
pub async fn exchange_code(
    endpoint: &dyn TokenEndpoint,
    request: &AuthRequest,
    client_id: &str,
    code: &str,
    now: Timestamp,
) -> Result<Tokens> {
    let params = vec![
        ("client_id".to_string(), client_id.to_string()),
        ("code".to_string(), code.to_string()),
        ("code_verifier".to_string(), request.verifier.clone()),
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("redirect_uri".to_string(), request.redirect_uri.clone()),
    ];

    let corps = endpoint.post_form(request.provider.token_url(), &params).await?;
    parse_tokens(&corps, now)
}

/// Renouvelle un jeton d'accès.
pub async fn refresh(
    endpoint: &dyn TokenEndpoint,
    provider: Provider,
    client_id: &str,
    refresh_token: &str,
    now: Timestamp,
) -> Result<Tokens> {
    let params = vec![
        ("client_id".to_string(), client_id.to_string()),
        ("refresh_token".to_string(), refresh_token.to_string()),
        ("grant_type".to_string(), "refresh_token".to_string()),
    ];

    let corps = endpoint.post_form(provider.token_url(), &params).await?;
    let mut jetons = parse_tokens(&corps, now)?;

    // Le fournisseur ne renvoie pas toujours le jeton de rafraîchissement : il faut
    // conserver l'ancien, sinon le compte se déconnecte au renouvellement suivant.
    if jetons.refresh_token.is_none() {
        jetons.refresh_token = Some(refresh_token.to_string());
    }
    Ok(jetons)
}

/// Analyse une réponse du point d'accès.
pub fn parse_tokens(body: &str, now: Timestamp) -> Result<Tokens> {
    let reponse: TokenResponse = serde_json::from_str(body)
        .map_err(|e| Error::Config(format!("réponse d'authentification illisible : {e}")))?;

    if let Some(erreur) = reponse.error {
        let details = reponse.error_description.unwrap_or_default();
        // Un jeton de rafraîchissement révoqué exige une reconnexion de
        // l'utilisateur : le distinguer d'une panne évite de marteler le serveur.
        if erreur == "invalid_grant" {
            return Err(Error::AuthFailed {
                account: format!("autorisation expirée ou révoquée : {details}"),
            });
        }
        return Err(Error::Config(format!("authentification refusée : {erreur} {details}")));
    }

    // Une heure est la valeur par défaut chez les deux fournisseurs.
    let duree = reponse.expires_in.unwrap_or(3600);

    Ok(Tokens {
        access_token: reponse.access_token,
        refresh_token: reponse.refresh_token,
        expires_at: Timestamp::from_millis((now.seconds() + duree) * 1000),
        email: reponse.id_token.as_deref().and_then(email_from_id_token),
    })
}

/// Extrait l'adresse d'un jeton d'identité, sans en vérifier la signature.
///
/// La vérification serait superflue ici : le jeton vient d'arriver par un canal TLS
/// direct avec le fournisseur, et il ne sert qu'à préremplir un champ d'affichage.
fn email_from_id_token(id_token: &str) -> Option<String> {
    let charge = id_token.split('.').nth(1)?;
    let octets = base64url_decode(charge)?;
    let json: serde_json::Value = serde_json::from_slice(&octets).ok()?;
    json.get("email")?.as_str().map(str::to_string)
}

// --- PKCE ---

/// Construit un vérificateur PKCE à partir d'une source d'aléa.
fn pkce_verifier(entropy: &[u8]) -> String {
    // Le RFC 7636 impose entre 43 et 128 caractères de l'alphabet non réservé.
    let mut brut = Vec::with_capacity(64);
    let mut etat = 0x6a09_e667_f3bc_c908u64;
    for (i, o) in entropy.iter().cycle().take(64).enumerate() {
        etat = etat.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(*o as u64 + i as u64);
        brut.push((etat >> 33) as u8);
    }
    base64url_encode(&brut)
}

/// Le défi est le condensé SHA-256 du vérificateur, encodé en base64url.
fn pkce_challenge(verifier: &str) -> String {
    base64url_encode(&sha256(verifier.as_bytes()))
}

fn state_token(entropy: &[u8]) -> String {
    let mut graine = entropy.to_vec();
    graine.extend_from_slice(b"state");
    base64url_encode(&sha256(&graine))
}

// --- Encodages ---

const B64URL: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn base64url_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for bloc in data.chunks(3) {
        let b = [bloc[0], *bloc.get(1).unwrap_or(&0), *bloc.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        let chiffres = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        // Pas de remplissage : le RFC 7636 l'interdit.
        let utiles = bloc.len() + 1;
        for c in chiffres.iter().take(utiles) {
            out.push(B64URL[*c as usize] as char);
        }
    }
    out
}

fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    let valeur = |c: u8| B64URL.iter().position(|x| *x == c).map(|v| v as u32);
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for bloc in s.as_bytes().chunks(4) {
        if bloc.len() < 2 {
            return None;
        }
        let mut n = 0u32;
        for (i, c) in bloc.iter().enumerate() {
            n |= valeur(*c)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if bloc.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if bloc.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for o in s.bytes() {
        match o {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(o as char)
            }
            _ => out.push_str(&format!("%{o:02X}")),
        }
    }
    out
}

fn urldecode(s: &str) -> String {
    let octets = s.replace('+', " ");
    let octets = octets.as_bytes();
    let mut out = Vec::with_capacity(octets.len());
    let mut i = 0;
    while i < octets.len() {
        if octets[i] == b'%' && i + 2 < octets.len() {
            if let Ok(v) = u8::from_str_radix(
                std::str::from_utf8(&octets[i + 1..i + 3]).unwrap_or(""),
                16,
            ) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(octets[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// SHA-256, implémenté ici pour ne pas dépendre d'une bibliothèque de plus.
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let mut message = data.to_vec();
    let longueur_bits = (data.len() as u64) * 8;
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&longueur_bits.to_be_bytes());

    for bloc in message.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([bloc[i * 4], bloc[i * 4 + 1], bloc[i * 4 + 2], bloc[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        for (i, v) in [a, b, c, d, e, f, g, hh].iter().enumerate() {
            h[i] = h[i].wrapping_add(*v);
        }
    }

    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct FakeEndpoint {
        reponse: Mutex<String>,
        derniere_requete: Mutex<Vec<(String, String)>>,
    }

    impl FakeEndpoint {
        fn with(reponse: &str) -> Self {
            Self {
                reponse: Mutex::new(reponse.to_string()),
                derniere_requete: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl TokenEndpoint for FakeEndpoint {
        async fn post_form(&self, _url: &str, params: &[(String, String)]) -> Result<String> {
            *self.derniere_requete.lock().unwrap() = params.to_vec();
            Ok(self.reponse.lock().unwrap().clone())
        }
    }

    fn now() -> Timestamp {
        Timestamp::from_millis(1_700_000_000_000)
    }

    #[test]
    fn sha256_donne_les_valeurs_de_reference() {
        // Vecteurs de test du NIST.
        let vide = sha256(b"");
        assert_eq!(
            vide[..4],
            [0xe3, 0xb0, 0xc4, 0x42],
            "condensé de la chaîne vide incorrect"
        );
        let abc = sha256(b"abc");
        assert_eq!(abc[..4], [0xba, 0x78, 0x16, 0xbf]);
    }

    #[test]
    fn base64url_fait_un_aller_retour() {
        for donnees in [&b""[..], b"a", b"ab", b"abc", b"abcd", &[0u8, 255, 128][..]] {
            let encode = base64url_encode(donnees);
            assert_eq!(base64url_decode(&encode).as_deref(), Some(donnees));
        }
    }

    #[test]
    fn base64url_n_utilise_pas_de_remplissage() {
        // Le RFC 7636 l'interdit explicitement.
        let encode = base64url_encode(b"a");
        assert!(!encode.contains('='));
        assert!(!encode.contains('+'));
        assert!(!encode.contains('/'));
    }

    #[test]
    fn le_verificateur_pkce_respecte_la_longueur_imposee() {
        let v = pkce_verifier(b"graine");
        assert!(v.len() >= 43 && v.len() <= 128, "longueur {} hors bornes", v.len());
        assert!(v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn deux_graines_donnent_deux_verificateurs() {
        assert_ne!(pkce_verifier(b"une"), pkce_verifier(b"autre"));
    }

    #[test]
    fn le_defi_est_le_condense_du_verificateur() {
        let v = pkce_verifier(b"graine");
        assert_eq!(pkce_challenge(&v), base64url_encode(&sha256(v.as_bytes())));
    }

    #[test]
    fn l_url_d_autorisation_contient_tout_le_necessaire() {
        let r = begin(Provider::Google, "client-123", 8080, Some("moi@gmail.com"), b"graine");
        assert!(r.url.starts_with("https://accounts.google.com/"));
        assert!(r.url.contains("client_id=client-123"));
        assert!(r.url.contains("code_challenge_method=S256"));
        assert!(r.url.contains("code_challenge="));
        assert!(r.url.contains("login_hint=moi%40gmail.com"));
        assert!(r.url.contains(&format!("state={}", urlencode(&r.state))));
    }

    #[test]
    fn google_exige_les_parametres_du_mode_hors_ligne() {
        // Sans eux, aucun jeton de rafraîchissement à la deuxième autorisation, et le
        // compte se déconnecte au bout d'une heure sans explication.
        let r = begin(Provider::Google, "c", 8080, None, b"g");
        assert!(r.url.contains("access_type=offline"));
        assert!(r.url.contains("prompt=consent"));

        let m = begin(Provider::Microsoft, "c", 8080, None, b"g");
        assert!(!m.url.contains("access_type"));
        assert!(m.url.contains("offline_access"));
    }

    #[test]
    fn la_redirection_pointe_vers_la_boucle_locale() {
        let r = begin(Provider::Google, "c", 7777, None, b"g");
        assert_eq!(r.redirect_uri, "http://127.0.0.1:7777/oauth");
    }

    #[test]
    fn le_code_est_extrait_de_la_redirection() {
        let r = begin(Provider::Google, "c", 8080, None, b"g");
        let url = format!("http://127.0.0.1:8080/oauth?code=abc123&state={}", r.state);
        assert_eq!(parse_redirect(&url, &r.state).unwrap(), "abc123");
    }

    #[test]
    fn un_jeton_anti_rejeu_incorrect_est_refuse() {
        // Sans cette comparaison, un site tiers pourrait faire aboutir sa propre
        // autorisation dans notre application.
        let url = "http://127.0.0.1:8080/oauth?code=abc&state=usurpe";
        let e = parse_redirect(url, "attendu").unwrap_err();
        assert!(e.to_string().contains("anti-rejeu"));
    }

    #[test]
    fn un_refus_d_autorisation_est_rapporte() {
        let url = "http://127.0.0.1:8080/oauth?error=access_denied&error_description=Refus%20utilisateur&state=s";
        let e = parse_redirect(url, "s").unwrap_err();
        assert!(e.to_string().contains("access_denied"));
        assert!(e.to_string().contains("Refus utilisateur"));
    }

    #[test]
    fn une_redirection_sans_code_est_refusee() {
        assert!(parse_redirect("http://x/?state=s", "s").is_err());
    }

    #[test]
    fn les_jetons_sont_analyses_avec_leur_peremption() {
        let corps = r#"{"access_token":"acces","refresh_token":"renouv","expires_in":3600}"#;
        let t = parse_tokens(corps, now()).unwrap();

        assert_eq!(t.access_token, "acces");
        assert_eq!(t.refresh_token.as_deref(), Some("renouv"));
        assert_eq!(t.expires_in_secs(now()), 3600);
        assert!(!t.is_expired(now(), 60));
    }

    #[test]
    fn la_marge_de_peremption_evite_l_expiration_en_cours_d_usage() {
        let corps = r#"{"access_token":"a","expires_in":30}"#;
        let t = parse_tokens(corps, now()).unwrap();
        assert!(!t.is_expired(now(), 10));
        assert!(t.is_expired(now(), 60), "une marge d'une minute doit le déclarer périmé");
    }

    #[test]
    fn une_duree_absente_retombe_sur_une_heure() {
        let t = parse_tokens(r#"{"access_token":"a"}"#, now()).unwrap();
        assert_eq!(t.expires_in_secs(now()), 3600);
    }

    #[test]
    fn une_autorisation_revoquee_demande_une_reconnexion() {
        let corps = r#"{"error":"invalid_grant","error_description":"Token has been expired"}"#;
        let e = parse_tokens(corps, now()).unwrap_err();
        assert!(e.needs_user_action(), "marteler le serveur serait inutile");
    }

    #[test]
    fn une_reponse_illisible_est_signalee() {
        assert!(parse_tokens("pas du json", now()).is_err());
    }

    #[tokio::test]
    async fn l_echange_transmet_le_verificateur_pkce() {
        let endpoint = FakeEndpoint::with(r#"{"access_token":"a","expires_in":3600}"#);
        let r = begin(Provider::Google, "client", 8080, None, b"g");

        exchange_code(&endpoint, &r, "client", "le-code", now()).await.unwrap();

        let requete = endpoint.derniere_requete.lock().unwrap().clone();
        let params: BTreeMap<_, _> = requete.into_iter().collect();
        assert_eq!(params.get("code").map(String::as_str), Some("le-code"));
        assert_eq!(params.get("code_verifier"), Some(&r.verifier));
        assert_eq!(params.get("grant_type").map(String::as_str), Some("authorization_code"));
    }

    #[tokio::test]
    async fn le_rafraichissement_conserve_l_ancien_jeton_de_renouvellement() {
        // Les fournisseurs ne le renvoient pas toujours ; l'oublier déconnecte le
        // compte au renouvellement suivant.
        let endpoint = FakeEndpoint::with(r#"{"access_token":"nouveau","expires_in":3600}"#);
        let t = refresh(&endpoint, Provider::Google, "client", "ancien-renouv", now())
            .await
            .unwrap();

        assert_eq!(t.access_token, "nouveau");
        assert_eq!(t.refresh_token.as_deref(), Some("ancien-renouv"));
    }

    #[tokio::test]
    async fn un_nouveau_jeton_de_renouvellement_remplace_l_ancien() {
        let endpoint =
            FakeEndpoint::with(r#"{"access_token":"a","refresh_token":"frais","expires_in":60}"#);
        let t = refresh(&endpoint, Provider::Google, "c", "ancien", now()).await.unwrap();
        assert_eq!(t.refresh_token.as_deref(), Some("frais"));
    }

    #[test]
    fn l_adresse_est_extraite_du_jeton_d_identite() {
        let charge = base64url_encode(br#"{"email":"marie@gmail.com","sub":"1"}"#);
        let id_token = format!("entete.{charge}.signature");
        assert_eq!(email_from_id_token(&id_token).as_deref(), Some("marie@gmail.com"));
    }

    #[test]
    fn un_jeton_d_identite_malforme_ne_fait_pas_paniquer() {
        assert!(email_from_id_token("pas.un.jeton").is_none());
        assert!(email_from_id_token("").is_none());
    }

    #[test]
    fn les_habilitations_demandees_restent_minimales() {
        // Demander davantage ferait hésiter l'utilisateur au moment le plus délicat.
        assert_eq!(Provider::Google.scopes().len(), 2);
        assert!(Provider::Microsoft.scopes().contains(&"offline_access"));
    }

    #[test]
    fn l_encodage_d_url_protege_les_caracteres_reserves() {
        assert_eq!(urlencode("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(urldecode("a%20b%26c"), "a b&c");
        assert_eq!(urldecode(&urlencode("https://x.fr/?a=1")), "https://x.fr/?a=1");
    }
}
