//! Templates: a note to start from, with what changes each time filled in.
//!
//! `{{title}}`, `{{date}}` (2026-10-07), `{{time}}` (14:32), `{{course}}`, and
//! `{{cursor}}`, where the cursor goes (taken out).

/// What a template is filled with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Values {
    pub title: String,
    pub date: String,
    pub time: String,
    pub course: String,
}

/// The template filled in, and where the cursor goes (the end when it does not say).
pub fn expand(template: &str, v: &Values) -> (String, usize) {
    let t = template
        .replace("{{title}}", &v.title)
        .replace("{{date}}", &v.date)
        .replace("{{time}}", &v.time)
        .replace("{{course}}", &v.course);
    match t.find("{{cursor}}") {
        Some(k) => (t.replacen("{{cursor}}", "", 1), k),
        None => {
            let n = t.len();
            (t, n)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_template_is_filled_in() {
        let v = Values {
            title: "Limites".into(),
            date: "2026-10-07".into(),
            time: "14:32".into(),
            course: "Analyse".into(),
        };
        let (t, c) = expand("# {{title}}\n{{course}}, {{date}}\n{{cursor}}\nfin", &v);
        assert_eq!(t, "# Limites\nAnalyse, 2026-10-07\n\nfin");
        assert_eq!(c, "# Limites\nAnalyse, 2026-10-07\n".len());
    }
}
