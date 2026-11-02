/// Shared lexical parsing keeps numeric terms exact and phrases together.
#[derive(Debug)]
struct Term {
    text: String,
    phrase: bool,
}
fn terms(q: &str) -> Vec<Term> {
    let mut result = Vec::new();
    let mut text = String::new();
    let mut quoted = false;
    for c in q.chars() {
        if c == '"' {
            if !text.is_empty() {
                result.push(Term { text: std::mem::take(&mut text).to_lowercase(), phrase: quoted });
            }
            quoted = !quoted;
        } else if quoted || c.is_alphanumeric() {
            text.push(c);
        } else if !text.is_empty() {
            result.push(Term { text: std::mem::take(&mut text).to_lowercase(), phrase: false });
        }
    }
    if !text.is_empty() {
        result.push(Term { text: text.to_lowercase(), phrase: quoted });
    }
    result.truncate(16);
    result
}
pub fn fts(q: &str) -> String {
    let terms = terms(q);
    let last = terms.len().saturating_sub(1);
    terms
        .iter()
        .enumerate()
        .map(|(i, t)| {
            format!(
                "\"{}\"{}",
                t.text.replace('"', "\"\""),
                if i == last && !t.phrase && !t.text.chars().all(char::is_numeric) { "*" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}
/// For searching contents: every word exactly (no prefix), and when there are several,
/// within a few words of each other.
pub fn fts_near(q: &str) -> String {
    let terms: Vec<String> = terms(q).iter().map(|t| format!("\"{}\"", t.text.replace('"', "\"\""))).collect();
    match terms.len() {
        0 => String::new(),
        1 => terms[0].clone(),
        _ => format!("NEAR({}, 5)", terms.join(" ")),
    }
}
pub fn name_rank(name: &str, q: &str) -> Option<u8> {
    let name = name.to_lowercase();
    let requested = terms(q);
    let q = q.trim().trim_matches('"').to_lowercase();
    if requested.is_empty() {
        return None;
    }
    let words: Vec<_> = name.split(|c: char| !c.is_alphanumeric()).filter(|s| !s.is_empty()).collect();
    let last = requested.len() - 1;
    if !requested.iter().enumerate().all(|(i, t)| {
        if t.phrase {
            let phrase: Vec<_> = t.text.split(|c: char| !c.is_alphanumeric()).filter(|s| !s.is_empty()).collect();
            return !phrase.is_empty() && words.windows(phrase.len()).any(|window| window == phrase);
        }
        words.iter().any(|w| *w == t.text || (i == last && !t.text.chars().all(char::is_numeric) && w.starts_with(&t.text)))
    }) {
        return None;
    }
    Some(if name == q {
        0
    } else if name.starts_with(&q) {
        1
    } else {
        2
    })
}
pub fn semantic_allowed(q: &str) -> bool {
    q.trim().starts_with("semantic:")
        || q.trim().starts_with("code:")
        || terms(q).iter().filter(|t| t.text.chars().any(char::is_alphabetic)).count() >= 2
}

/// Lower is better. Relevance tiers always precede personal usage bonuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Quality(pub u8, pub u16);

/// Construct once per query; names passed to `score_lower` are cached lowercase text.
pub struct Matcher {
    text: String,
    requested: Vec<Term>,
}
impl Matcher {
    pub fn new(q: &str) -> Self {
        Self { text: q.trim().trim_matches('"').to_lowercase(), requested: terms(q) }
    }
    pub fn score(&self, name: &str) -> Option<Quality> {
        self.score_lower(&name.to_lowercase())
    }
    pub fn score_lower(&self, name: &str) -> Option<Quality> {
        if self.requested.is_empty() || self.text.chars().count() > 256 {
            return None;
        }
        if name == self.text {
            return Some(Quality(1, 0));
        }
        let words: Vec<_> = name.split(|c: char| !c.is_alphanumeric()).filter(|s| !s.is_empty()).collect();
        // Initialisms are intentional abbreviations, not arbitrary one-letter matches.
        if self.requested.len() == 1
            && !self.requested[0].phrase
            && self.text.len() >= 2
            && self.text.chars().all(char::is_alphabetic)
            && words.iter().filter_map(|w| w.chars().next()).collect::<String>() == self.text
        {
            return Some(Quality(5, 0));
        }
        let mut quality = Quality(if name.starts_with(&self.text) { 3 } else { 4 }, 0);
        for (i, term) in self.requested.iter().enumerate() {
            if term.phrase {
                let phrase: Vec<_> = term.text.split(|c: char| !c.is_alphanumeric()).filter(|s| !s.is_empty()).collect();
                if phrase.is_empty() || !words.windows(phrase.len()).any(|w| w == phrase) {
                    return None;
                }
                continue;
            }
            let numeric = term.text.chars().all(char::is_numeric);
            if words.iter().any(|w| *w == term.text || (!numeric && i + 1 == self.requested.len() && w.starts_with(&term.text))) {
                continue;
            }
            if numeric {
                return None;
            }
            let q: Vec<char> = term.text.chars().collect();
            let best = words
                .iter()
                .filter_map(|word| {
                    let w: Vec<char> = word.chars().collect();
                    // Subsequence matching stays within a word and starts at its beginning.
                    if q.len() >= 3 && w.first() == q.first() && q.len() * 2 >= w.len() {
                        let mut at = 0;
                        for ch in &w {
                            if q.get(at) == Some(ch) {
                                at += 1;
                            }
                        }
                        if at == q.len() {
                            return Some(Quality(6, (w.len() - q.len()) as u16));
                        }
                    }
                    let limit = if q.len() >= 8 {
                        2
                    } else if q.len() >= 4 {
                        1
                    } else {
                        0
                    };
                    if limit == 0 || w.len().abs_diff(q.len()) > limit {
                        return None;
                    }
                    let edits = edit_distance(&q, &w, limit);
                    (edits <= limit).then_some(Quality(7, edits as u16))
                })
                .min()?;
            quality = Quality(quality.0.max(best.0), quality.1.saturating_add(best.1));
        }
        Some(quality)
    }
}
fn edit_distance(a: &[char], b: &[char], limit: usize) -> usize {
    let mut prev2 = vec![0; b.len() + 1];
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut row = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        row[0] = i;
        for j in 1..=b.len() {
            row[j] = (prev[j] + 1).min(row[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                row[j] = row[j].min(prev2[j - 2] + 1);
            }
        }
        if *row.iter().min().unwrap_or(&0) > limit {
            return limit + 1;
        }
        std::mem::swap(&mut prev2, &mut prev);
        std::mem::swap(&mut prev, &mut row);
    }
    prev[b.len()]
}
