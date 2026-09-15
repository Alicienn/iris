//! Pourquoi le trousseau système répond non.
//!
//! [`KeyringStore::is_available`] rend un booléen, ce qui suffit à décider et pas à
//! comprendre. Sur une machine où le coffre chiffré prend le relais sans raison
//! apparente — et où l'application se met alors à demander un mot de passe maître au
//! démarrage, sur une entrée standard qui n'existe pas quand on lance depuis un
//! raccourci — cet exemple est la seule chose qui dise ce qui se passe réellement.
//!
//! ```text
//! cargo run -p iris-secrets --example sonde
//! ```

fn main() {
    match keyring::Entry::new("Iris", "__iris_probe__:password") {
        Err(e) => println!("Entry::new -> {e:?}"),
        Ok(entree) => println!("get_password -> {:?}", entree.get_password()),
    }
}
