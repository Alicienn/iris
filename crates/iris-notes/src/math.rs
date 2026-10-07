//! LaTeX maths as Unicode text, for a line that cannot draw it: `\alpha^2` reads `α²`,
//! `\frac{a}{b}` reads `a/b`, `\mathbb{R}` reads `ℝ`.
//!
//! Display maths is drawn as an image; this is for maths inside a line, which is short
//! in notes ("soit $f$ continue sur $[a,b]$") and reads well this way. What has no
//! Unicode form is left as typed.

use crate::inline::raise;

/// A command's Unicode form.
fn symbole(nom: &str) -> Option<&'static str> {
    Some(match nom {
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" | "varepsilon" => "ε",
        "zeta" => "ζ",
        "eta" => "η",
        "theta" | "vartheta" => "θ",
        "iota" => "ι",
        "kappa" => "κ",
        "lambda" => "λ",
        "mu" => "μ",
        "nu" => "ν",
        "xi" => "ξ",
        "pi" | "varpi" => "π",
        "rho" | "varrho" => "ρ",
        "sigma" | "varsigma" => "σ",
        "tau" => "τ",
        "upsilon" => "υ",
        "phi" | "varphi" => "φ",
        "chi" => "χ",
        "psi" => "ψ",
        "omega" => "ω",
        "Gamma" => "Γ",
        "Delta" => "Δ",
        "Theta" => "Θ",
        "Lambda" => "Λ",
        "Xi" => "Ξ",
        "Pi" => "Π",
        "Sigma" => "Σ",
        "Upsilon" => "Υ",
        "Phi" => "Φ",
        "Psi" => "Ψ",
        "Omega" => "Ω",
        "infty" => "∞",
        "leq" | "le" => "≤",
        "geq" | "ge" => "≥",
        "neq" | "ne" => "≠",
        "approx" => "≈",
        "equiv" => "≡",
        "sim" => "∼",
        "simeq" => "≃",
        "cong" => "≅",
        "propto" => "∝",
        "ll" => "≪",
        "gg" => "≫",
        "to" | "rightarrow" => "→",
        "leftarrow" | "gets" => "←",
        "Rightarrow" | "implies" => "⇒",
        "Leftarrow" => "⇐",
        "Leftrightarrow" | "iff" => "⇔",
        "leftrightarrow" => "↔",
        "mapsto" => "↦",
        "uparrow" => "↑",
        "downarrow" => "↓",
        "in" => "∈",
        "notin" => "∉",
        "ni" => "∋",
        "subset" => "⊂",
        "subseteq" => "⊆",
        "supset" => "⊃",
        "supseteq" => "⊇",
        "cup" => "∪",
        "cap" => "∩",
        "setminus" => "∖",
        "emptyset" | "varnothing" => "∅",
        "forall" => "∀",
        "exists" => "∃",
        "nexists" => "∄",
        "neg" | "lnot" => "¬",
        "wedge" | "land" => "∧",
        "vee" | "lor" => "∨",
        "partial" => "∂",
        "nabla" => "∇",
        "int" => "∫",
        "iint" => "∬",
        "iiint" => "∭",
        "oint" => "∮",
        "sum" => "∑",
        "prod" => "∏",
        "coprod" => "∐",
        "pm" => "±",
        "mp" => "∓",
        "times" => "×",
        "cdot" => "·",
        "div" => "÷",
        "circ" => "∘",
        "bullet" => "•",
        "star" => "⋆",
        "ast" => "∗",
        "oplus" => "⊕",
        "otimes" => "⊗",
        "ldots" | "dots" => "…",
        "cdots" => "⋯",
        "vdots" => "⋮",
        "ddots" => "⋱",
        "perp" => "⊥",
        "parallel" => "∥",
        "angle" => "∠",
        "degree" => "°",
        "prime" => "′",
        "langle" => "⟨",
        "rangle" => "⟩",
        "lfloor" => "⌊",
        "rfloor" => "⌋",
        "lceil" => "⌈",
        "rceil" => "⌉",
        "mid" => "∣",
        "vert" => "|",
        "Vert" | "|" => "‖",
        "hbar" => "ℏ",
        "ell" => "ℓ",
        "Re" => "ℜ",
        "Im" => "ℑ",
        "aleph" => "ℵ",
        "top" => "⊤",
        "bot" => "⊥",
        "vdash" => "⊢",
        "models" => "⊨",
        "therefore" => "∴",
        "because" => "∵",
        "lbrace" | "{" => "{",
        "rbrace" | "}" => "}",
        "%" => "%",
        "$" => "$",
        "&" => "&",
        "_" => "_",
        "#" => "#",
        "," | ";" | ":" | " " | "quad" => " ",
        "qquad" => "  ",
        "!" => "",
        _ => return None,
    })
}

/// Functions written upright: `\sin x` reads `sin x`.
const FONCTIONS: &[&str] = &[
    "sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos", "arctan", "sinh", "cosh", "tanh",
    "log", "ln", "lg", "exp", "lim", "liminf", "limsup", "max", "min", "sup", "inf", "det", "dim",
    "ker", "deg", "gcd", "arg", "Pr", "mod", "bmod",
];

/// A letter in a double-struck, calligraphic or bold face.
fn police(face: &str, c: char) -> Option<char> {
    match face {
        "mathbb" => Some(match c {
            'R' => 'ℝ',
            'N' => 'ℕ',
            'Z' => 'ℤ',
            'Q' => 'ℚ',
            'C' => 'ℂ',
            'P' => 'ℙ',
            'H' => 'ℍ',
            'E' => '𝔼',
            'K' => '𝕂',
            'F' => '𝔽',
            '1' => '𝟙',
            _ => return None,
        }),
        "mathcal" => match c {
            'A'..='Z' => {
                let speciaux = [
                    ('B', 'ℬ'),
                    ('E', 'ℰ'),
                    ('F', 'ℱ'),
                    ('H', 'ℋ'),
                    ('I', 'ℐ'),
                    ('L', 'ℒ'),
                    ('M', 'ℳ'),
                    ('R', 'ℛ'),
                ];
                speciaux
                    .iter()
                    .find(|(l, _)| *l == c)
                    .map(|(_, s)| *s)
                    .or_else(|| char::from_u32(0x1D49C + (c as u32 - 'A' as u32)))
            }
            _ => None,
        },
        _ => None,
    }
}

/// A reader over the LaTeX source.
struct Lecteur<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Lecteur<'a> {
    fn fini(&self) -> bool {
        self.i >= self.s.len()
    }
    fn regarder(&self) -> Option<char> {
        self.s[self.i..].chars().next()
    }
    fn prendre(&mut self) -> Option<char> {
        let c = self.regarder()?;
        self.i += c.len_utf8();
        Some(c)
    }
    fn espaces(&mut self) {
        while self.regarder().is_some_and(char::is_whitespace) {
            self.i += 1;
        }
    }
    /// A command's name after `\`: letters, or one other character.
    fn commande(&mut self) -> &'a str {
        let debut = self.i;
        while self.regarder().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.i += 1;
        }
        if self.i == debut {
            self.prendre();
        }
        &self.s[debut..self.i]
    }
    /// `{…}` as raw source, or the next single token.
    fn argument(&mut self) -> &'a str {
        self.espaces();
        match self.regarder() {
            Some('{') => {
                self.i += 1;
                let debut = self.i;
                let mut profondeur = 1;
                while let Some(c) = self.prendre() {
                    match c {
                        '\\' => {
                            self.prendre();
                        }
                        '{' => profondeur += 1,
                        '}' => {
                            profondeur -= 1;
                            if profondeur == 0 {
                                return &self.s[debut..self.i - 1];
                            }
                        }
                        _ => {}
                    }
                }
                &self.s[debut..]
            }
            Some('\\') => {
                let debut = self.i;
                self.i += 1;
                self.commande();
                &self.s[debut..self.i]
            }
            Some(c) => {
                let debut = self.i;
                self.i += c.len_utf8();
                &self.s[debut..self.i]
            }
            None => "",
        }
    }
    /// `[…]`, an optional argument, when there is one.
    fn optionnel(&mut self) -> Option<&'a str> {
        if self.regarder() != Some('[') {
            return None;
        }
        let debut = self.i + 1;
        let fin = self.s[debut..].find(']')? + debut;
        self.i = fin + 1;
        Some(&self.s[debut..fin])
    }
}

/// Whether a piece needs brackets around it to read as one thing: `a+b` in `(a+b)/2`.
fn compose(s: &str) -> bool {
    s.chars().count() > 1
        && s.chars()
            .any(|c| matches!(c, '+' | '-' | '−' | '·' | '×' | ' ' | '/' | ','))
}

fn entoure(s: &str) -> String {
    if compose(s) {
        format!("({s})")
    } else {
        s.to_string()
    }
}

/// The LaTeX of a formula, read as Unicode text.
pub fn to_unicode(latex: &str) -> String {
    let mut l = Lecteur { s: latex, i: 0 };
    let mut sortie = String::with_capacity(latex.len());
    while !l.fini() {
        let Some(c) = l.prendre() else { break };
        match c {
            '\\' => {
                let nom = l.commande();
                ecrire_commande(nom, &mut l, &mut sortie);
            }
            '^' | '_' => {
                let arg = to_unicode(l.argument());
                sortie.push_str(&raise(&arg, c == '^'));
            }
            '{' | '}' => {}
            '~' => sortie.push(' '),
            '-' => sortie.push('−'),
            '*' => sortie.push('∗'),
            c => sortie.push(c),
        }
    }
    sortie
}

fn ecrire_commande(nom: &str, l: &mut Lecteur<'_>, sortie: &mut String) {
    match nom {
        "frac" | "dfrac" | "tfrac" => {
            let num = to_unicode(l.argument());
            let den = to_unicode(l.argument());
            sortie.push_str(&format!("{}/{}", entoure(&num), entoure(&den)));
        }
        "sqrt" => {
            let indice = l.optionnel().map(to_unicode);
            let rad = to_unicode(l.argument());
            let racine = match indice.as_deref() {
                Some("3") => "∛",
                Some("4") => "∜",
                _ => "√",
            };
            if let Some(i) = indice.filter(|i| i != "3" && i != "4") {
                sortie.push_str(&raise(&i, true));
            }
            sortie.push_str(racine);
            sortie.push_str(&entoure(&rad));
        }
        "text" | "textrm" | "mathrm" | "operatorname" | "mathbf" | "boldsymbol" | "textbf"
        | "mathit" | "mathsf" | "mathtt" => {
            sortie.push_str(&to_unicode(l.argument()));
        }
        "mathbb" | "mathcal" | "mathscr" => {
            let face = if nom == "mathscr" { "mathcal" } else { nom };
            for c in l.argument().chars() {
                sortie.push(police(face, c).unwrap_or(c));
            }
        }
        "vec" | "overrightarrow" => accent(l, sortie, '\u{20D7}'),
        "hat" | "widehat" => accent(l, sortie, '\u{0302}'),
        "bar" | "overline" => accent(l, sortie, '\u{0305}'),
        "tilde" | "widetilde" => accent(l, sortie, '\u{0303}'),
        "dot" => accent(l, sortie, '\u{0307}'),
        "ddot" => accent(l, sortie, '\u{0308}'),
        "left" | "right" | "big" | "Big" | "bigg" | "Bigg" | "displaystyle" | "limits"
        | "nolimits" | "middle" => {}
        "begin" | "end" => {
            // An environment's name is not read; its rows are.
            l.argument();
        }
        "\\" => sortie.push_str("; "),
        _ if FONCTIONS.contains(&nom) => {
            sortie.push_str(nom);
            if l.regarder().is_some_and(|c| c.is_alphanumeric()) {
                sortie.push(' ');
            }
        }
        _ => match symbole(nom) {
            Some(s) => sortie.push_str(s),
            None => {
                sortie.push('\\');
                sortie.push_str(nom);
            }
        },
    }
}

/// A letter with a combining mark over it (`x⃗`); over several letters, each gets it.
fn accent(l: &mut Lecteur<'_>, sortie: &mut String, marque: char) {
    let arg = to_unicode(l.argument());
    for c in arg.chars() {
        sortie.push(c);
        if c.is_alphanumeric() {
            sortie.push(marque);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formulas_read_as_text() {
        assert_eq!(to_unicode(r"\alpha^2 + \beta_i"), "α² + βᵢ");
        assert_eq!(to_unicode(r"\frac{a+b}{2}"), "(a+b)/2");
        assert_eq!(to_unicode(r"\frac{1}{n}"), "1/n");
        assert_eq!(to_unicode(r"\sqrt{x}"), "√x");
        assert_eq!(to_unicode(r"\sqrt[3]{8}"), "∛8");
        assert_eq!(to_unicode(r"x \in \mathbb{R}"), "x ∈ ℝ");
        assert_eq!(to_unicode(r"\forall \epsilon > 0"), "∀ ε > 0");
        assert_eq!(to_unicode(r"\lim_{n \to \infty} u_n"), "lim_(n → ∞) uₙ");
        assert_eq!(to_unicode(r"\sin x"), "sin x");
        assert_eq!(to_unicode(r"\vec{u}"), "u\u{20D7}");
        assert_eq!(to_unicode(r"\text{si } x"), "si  x");
        assert_eq!(to_unicode(r"a - b"), "a − b");
        assert_eq!(to_unicode(r"\unknown"), r"\unknown");
        assert_eq!(to_unicode(r"\left( x \right)"), "( x )");
    }
}
