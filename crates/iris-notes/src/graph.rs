//! The graph of a space: its notes placed so that linked notes sit close and the
//! others apart (Fruchterman and Reingold's forces), in a square from 0 to 1.

/// Places `n` notes linked by `edges` (pairs of indices). The same input gives the
/// same picture: the notes start on a spiral, not at random.
pub fn layout(n: usize, edges: &[(usize, usize)]) -> Vec<(f32, f32)> {
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![(0.5, 0.5)];
    }
    // Start: a sunflower spiral, even and deterministic.
    let mut pos: Vec<(f32, f32)> = (0..n)
        .map(|i| {
            let r = ((i as f32 + 0.5) / n as f32).sqrt() * 0.45;
            let a = i as f32 * 2.399_963;
            (0.5 + r * a.cos(), 0.5 + r * a.sin())
        })
        .collect();
    let k = (1.0 / n as f32).sqrt() * 0.75;
    // Fewer rounds for big spaces: the work grows with the square of the notes.
    let tours = if n > 600 {
        40
    } else if n > 250 {
        80
    } else {
        150
    };
    let mut temperature = 0.1_f32;
    let refroidir = temperature / tours as f32;
    for _ in 0..tours {
        let mut depl = vec![(0.0_f32, 0.0_f32); n];
        // Every note pushes every other away.
        for (i, a) in pos.iter().enumerate() {
            for (j, b) in pos.iter().enumerate().skip(i + 1) {
                let dx = a.0 - b.0;
                let dy = a.1 - b.1;
                let d2 = (dx * dx + dy * dy).max(1e-6);
                let f = k * k / d2;
                depl[i].0 += dx * f;
                depl[i].1 += dy * f;
                depl[j].0 -= dx * f;
                depl[j].1 -= dy * f;
            }
        }
        // A link pulls its two notes together.
        for &(a, b) in edges {
            if a >= n || b >= n || a == b {
                continue;
            }
            let dx = pos[a].0 - pos[b].0;
            let dy = pos[a].1 - pos[b].1;
            let d = (dx * dx + dy * dy).sqrt().max(1e-4);
            let f = d / k;
            depl[a].0 -= dx * f;
            depl[a].1 -= dy * f;
            depl[b].0 += dx * f;
            depl[b].1 += dy * f;
        }
        // And everything a little toward the middle, so islands do not drift away.
        for (p, d) in pos.iter_mut().zip(&depl) {
            let (mut dx, mut dy) = *d;
            dx += (0.5 - p.0) * 0.6;
            dy += (0.5 - p.1) * 0.6;
            let l = (dx * dx + dy * dy).sqrt().max(1e-6);
            let pas = l.min(temperature);
            p.0 += dx / l * pas;
            p.1 += dy / l * pas;
        }
        temperature = (temperature - refroidir).max(0.002);
    }
    // Fitted to the square, with a margin.
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in &pos {
        x0 = x0.min(p.0);
        y0 = y0.min(p.1);
        x1 = x1.max(p.0);
        y1 = y1.max(p.1);
    }
    let echelle = (x1 - x0).max(y1 - y0).max(1e-6);
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    pos.into_iter()
        .map(|(x, y)| {
            (
                0.5 + (x - cx) / echelle * 0.88,
                0.5 + (y - cy) / echelle * 0.88,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
        ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
    }

    #[test]
    fn linked_notes_sit_closer_than_the_others() {
        // Two groups of three, linked inside, not between.
        let liens = [(0, 1), (1, 2), (2, 0), (3, 4), (4, 5), (5, 3)];
        let p = layout(6, &liens);
        assert_eq!(p.len(), 6);
        assert!(p
            .iter()
            .all(|(x, y)| (0.0..=1.0).contains(x) && (0.0..=1.0).contains(y)));
        let dedans = distance(p[0], p[1]);
        let entre = distance(p[0], p[4]);
        assert!(dedans < entre, "{dedans} ≥ {entre}");
        // The same every time.
        assert_eq!(layout(6, &liens), p);
    }

    #[test]
    fn nothing_one_and_bad_links() {
        assert!(layout(0, &[]).is_empty());
        assert_eq!(layout(1, &[(0, 0)]), vec![(0.5, 0.5)]);
        assert_eq!(layout(3, &[(0, 9), (1, 1)]).len(), 3);
    }
}
