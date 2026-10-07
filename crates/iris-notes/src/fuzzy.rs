//! Finding a note by its name, typed loosely: `chap1` finds "Chapitre 1", `anal` finds
//! "Analyse", `lim seq` finds "Limites de suites".
//!
//! Case and accents do not count. A name that starts with what was typed comes first,
//! then one where each word typed starts a word, then one holding it all in order.

/// A letter folded for comparing: lower case, no accent.
pub fn fold(c: char) -> char {
    let c = match c {
        'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' | 'À' | 'Â' | 'Ä' | 'Á' | 'Ã' | 'Å' => 'a',
        'ç' | 'Ç' => 'c',
        'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'e',
        'î' | 'ï' | 'í' | 'ì' | 'Î' | 'Ï' | 'Í' | 'Ì' => 'i',
        'ô' | 'ö' | 'ó' | 'ò' | 'õ' | 'Ô' | 'Ö' | 'Ó' | 'Ò' | 'Õ' => 'o',
        'ù' | 'û' | 'ü' | 'ú' | 'Ù' | 'Û' | 'Ü' | 'Ú' => 'u',
        'ÿ' => 'y',
        'ñ' | 'Ñ' => 'n',
        c => c,
    };
    c.to_lowercase().next().unwrap_or(c)
}

/// A text folded whole.
pub fn folded(s: &str) -> String {
    s.chars().map(fold).collect()
}

/// How well `name` matches `query`; `None` when it does not. Higher is better.
pub fn score(query: &str, name: &str) -> Option<i32> {
    let q = folded(query.trim());
    if q.is_empty() {
        return Some(0);
    }
    let n = folded(name);
    if n == q {
        return Some(1000);
    }
    if n.starts_with(&q) {
        return Some(800 - n.len() as i32);
    }
    // Each word typed starts a word of the name, in order.
    let mots_n: Vec<&str> = n
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let mots_q: Vec<&str> = q.split_whitespace().collect();
    let mut k = 0;
    let mut tous = true;
    for mq in &mots_q {
        match mots_n[k..].iter().position(|w| w.starts_with(mq)) {
            Some(p) => k += p + 1,
            None => {
                tous = false;
                break;
            }
        }
    }
    if tous {
        return Some(600 - n.len() as i32);
    }
    if n.contains(&q) {
        return Some(400 - n.len() as i32);
    }
    // Every letter typed, in order (spaces left out).
    let mut lettres = n.chars();
    let mut ecarts = 0;
    for c in q.chars().filter(|c| !c.is_whitespace()) {
        let mut trouve = false;
        for x in lettres.by_ref() {
            if x == c {
                trouve = true;
                break;
            }
            ecarts += 1;
        }
        if !trouve {
            return None;
        }
    }
    Some(200 - ecarts.min(150) - n.len() as i32 / 4)
}

/// The best matches of `query` among `names`, best first: their indices.
pub fn rank(query: &str, names: &[&str], max: usize) -> Vec<usize> {
    let mut notes: Vec<(i32, usize)> = names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| score(query, n).map(|s| (s, i)))
        .collect();
    notes.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    notes.into_iter().take(max).map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_come_first_then_words_then_letters() {
        let noms = [
            "Limites de suites",
            "Analyse",
            "Chapitre 1",
            "Algèbre linéaire",
            "Calcul",
        ];
        assert_eq!(rank("anal", &noms, 5)[0], 1);
        assert_eq!(rank("lim sui", &noms, 5)[0], 0);
        assert_eq!(rank("chap1", &noms, 5)[0], 2);
        assert_eq!(rank("algebre", &noms, 5)[0], 3, "accents do not count");
        assert!(score("xyz", "Analyse").is_none());
        assert!(score("Analyse", "analyse").unwrap() > score("Ana", "Analyse").unwrap());
    }
}
