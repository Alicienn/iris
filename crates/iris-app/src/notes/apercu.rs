//! What each block of the `/` menu looks like, for the card beside it: what is typed,
//! what it becomes, and a line saying what it is for.

/// The card of one block.
#[derive(Debug, Clone, PartialEq)]
pub struct Apercu {
    /// How the result is drawn: "heading", "bullet", "numbered", "task", "quote",
    /// "callout", "math", "code", "table", "rule", "cols", "fold", "card", "sheet".
    pub family: &'static str,
    /// What is typed to make it.
    pub source: &'static str,
    /// The result's first line (a heading's words, a callout's title…).
    pub title: String,
    /// The result's words under it.
    pub body: &'static str,
    pub description: &'static str,
    /// A heading's level; a callout folded (1) or not (0).
    pub level: i32,
    /// A callout's colour.
    pub tint: slint::Color,
}

fn simple(
    family: &'static str,
    source: &'static str,
    title: &str,
    body: &'static str,
    description: &'static str,
) -> Apercu {
    Apercu {
        family,
        source,
        title: title.to_string(),
        body,
        description,
        level: 0,
        tint: slint::Color::default(),
    }
}

/// A callout's card: its label and colour as the note draws them.
fn encadre(
    genre: &str,
    source: &'static str,
    titre: &str,
    body: &'static str,
    description: &'static str,
    plie: bool,
) -> Apercu {
    let (label, tint, numerote) = super::render::callout_label(genre);
    let entete = match (numerote, titre.is_empty()) {
        (true, false) => format!("{label} 1 — {titre}"),
        (true, true) => format!("{label} 1"),
        (false, false) => format!("{label} — {titre}"),
        (false, true) => label.to_string(),
    };
    Apercu {
        family: "callout",
        source,
        title: entete,
        body,
        description,
        level: i32::from(plie),
        tint,
    }
}

/// The card of the block `key` of the `/` menu (`h1`, `thm`, `table`…).
pub fn apercu(key: &str) -> Option<Apercu> {
    Some(match key {
        "h1" | "h2" | "h3" => {
            let n = key[1..].parse::<i32>().unwrap_or(1);
            let mut a = simple(
                "heading",
                ["# Limits", "## Sequences", "### Monotone sequences"][(n - 1) as usize],
                ["Limits", "Sequences", "Monotone sequences"][(n - 1) as usize],
                "",
                [
                    "The title of a part. Type # and a space; Ctrl+H steps through the levels.",
                    "The title of a section, under a part. Type ## and a space.",
                    "A smaller title, for a point inside a section. Type ### and a space.",
                ][(n - 1) as usize],
            );
            a.level = n;
            a
        }
        "list" => simple(
            "bullet",
            "- First idea",
            "First idea",
            "Second idea",
            "A list of points. Enter goes on, Tab nests, Enter on an empty point ends it.",
        ),
        "num" => simple(
            "numbered",
            "1. Read the statement",
            "Read the statement",
            "Write what is known",
            "Steps in order, numbered as they go. Type 1. and a space.",
        ),
        "todo" => simple(
            "task",
            "[] Redo exercise 3",
            "Redo exercise 3",
            "Read chapter 2",
            "Something to do, ticked with a click or Ctrl+Enter. Ctrl+Shift+T makes it a task.",
        ),
        "quote" => simple(
            "quote",
            "> Nothing is lost",
            "Nothing is lost, nothing is created.",
            "",
            "Words quoted from someone else, set apart by a bar.",
        ),
        "def" => encadre(
            "def",
            ">def Limit",
            "Limit",
            "(uₙ) tends to ℓ if it gets as close as wanted.",
            "A definition, numbered in the note; [[#def-1]] links to it.",
            false,
        ),
        "thm" => encadre(
            "thm",
            ">thm Pythagoras",
            "Pythagoras",
            "a² + b² = c² in a right triangle.",
            "A theorem, numbered in the note; [[#thm-1]] links to it.",
            false,
        ),
        "prop" => encadre(
            "prop",
            ">prop Uniqueness",
            "Uniqueness",
            "A convergent sequence has one limit.",
            "A proposition, numbered with its kind.",
            false,
        ),
        "lem" => encadre(
            "lem",
            ">lem Bound",
            "Bound",
            "A convergent sequence is bounded.",
            "A lemma: a step toward a theorem, numbered.",
            false,
        ),
        "cor" => encadre(
            "cor",
            ">cor Monotone",
            "Monotone",
            "Increasing and bounded: it converges.",
            "A corollary: what follows from a theorem, numbered.",
            false,
        ),
        "proof" => encadre(
            "proof",
            ">-proof",
            "",
            "Let ε > 0…",
            "A proof, folded: its arrow opens it when you want to read it.",
            true,
        ),
        "ex" => encadre(
            "example",
            ">ex Harmonic",
            "Harmonic",
            "1 + 1/2 + 1/3 + … diverges.",
            "An example, in green, numbered.",
            false,
        ),
        "exo" => encadre(
            "exo",
            ">exo Show it",
            "Show it",
            "Prove that √2 is irrational.",
            "An exercise, numbered, to come back to.",
            false,
        ),
        "sol" => encadre(
            "sol",
            ">-sol",
            "",
            "Suppose √2 = p/q…",
            "A solution, folded so it is not read by accident.",
            true,
        ),
        "q" => encadre(
            "question",
            ">q Why?",
            "Why does it converge?",
            "Because it is increasing and bounded.",
            "A question and its answer; it is also a flashcard to revise.",
            false,
        ),
        "important" => encadre(
            "important",
            ">! Exam",
            "Exam",
            "Chapters 1 to 4, on Friday.",
            "What must not be missed, in red.",
            false,
        ),
        "summary" => encadre(
            "summary",
            ">res",
            "",
            "Increasing + bounded ⇒ convergent.",
            "What to remember of a part, in a few lines.",
            false,
        ),
        "note" => encadre(
            "note",
            ">note",
            "",
            "See also chapter 5.",
            "An aside, in blue.",
            false,
        ),
        "tip" => encadre(
            "tip",
            ">tip",
            "",
            "Draw the triangle first.",
            "A piece of advice, in green.",
            false,
        ),
        "warn" => encadre(
            "warning",
            ">warn",
            "",
            "Do not divide by zero.",
            "A trap to avoid, in orange.",
            false,
        ),
        "math" => simple(
            "math",
            "$$ \\frac{a+b}{2} $$",
            "\\int_0^1 x^2\\,dx = \\frac{1}{3}",
            "∫₀¹ x² dx = 1/3",
            "A formula on a line of its own, written in LaTeX and drawn as in a book.",
        ),
        "code" => simple(
            "code",
            "```python",
            "def mean(xs):",
            "    return sum(xs) / len(xs)",
            "Code, in a fixed-width font on its own plate.",
        ),
        "table" => simple(
            "table",
            "| A | B |",
            "",
            "",
            "A table typed in Markdown; for formulas, use a spreadsheet.",
        ),
        "rule" => simple(
            "rule",
            "---",
            "",
            "",
            "A line across the page, between two parts.",
        ),
        "cols" => simple(
            "cols",
            ":::cols",
            "Left",
            "Right",
            "Two columns side by side, split by :::col.",
        ),
        "fold" => simple(
            "fold",
            ":::fold Details",
            "Details",
            "Hidden until opened.",
            "A block folded under its title, opened with its arrow.",
        ),
        "card" => simple(
            "card",
            "2 + 2 ? :: 4",
            "2 + 2 ?",
            "4",
            "A flashcard: the question, ::, the answer. Revise brings it back in time.",
        ),
        "sheet" => simple(
            "sheet",
            "/sheet",
            "Budget",
            "",
            "A spreadsheet made beside the note and shown in it; a click opens it.",
        ),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_block_of_the_menu_has_its_card() {
        for (k, nom, _, _) in iris_notes::complete::BLOCKS {
            let a = apercu(k).unwrap_or_else(|| panic!("no card for {k} ({nom})"));
            assert!(!a.description.is_empty() && !a.source.is_empty(), "{k}");
        }
        assert_eq!(apercu("thm").unwrap().title, "Theorem 1 — Pythagoras");
        assert_eq!(apercu("proof").unwrap().level, 1);
        assert!(apercu("nope").is_none());
    }
}
