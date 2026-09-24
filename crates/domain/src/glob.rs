//! Path patterns for finding compose files in a repository.
//!
//! Small on purpose: `*` and `?` within one directory, `**` across any
//! number of them, `{a,b}` for alternatives, and commas between whole
//! patterns. Matching never backtracks exponentially, because the pattern
//! comes from a person typing into a form and the paths from a repository
//! nobody vetted.

/// Most alternatives a pattern may expand to. `{a,b}{c,d}{e,f}…` doubles
/// with each group; this stops a typo becoming a denial of service.
const MAX_ALTERNATIVES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatternError {
    #[error("the pattern is empty")]
    Empty,
    #[error("a {{ is not closed, or a }} was not opened")]
    Unbalanced,
    #[error("the pattern expands to more than {MAX_ALTERNATIVES} alternatives")]
    TooMany,
}

/// One or more patterns; a path matches if any of them does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    alternatives: Vec<Vec<String>>,
}

impl Pattern {
    pub fn parse(source: &str) -> Result<Self, PatternError> {
        let mut alternatives = Vec::new();
        for piece in split_top_level(source)? {
            let piece = piece.trim();
            if piece.is_empty() {
                return Err(PatternError::Empty);
            }
            for expanded in expand(piece)? {
                alternatives.push(expanded.split('/').map(str::to_owned).collect());
                if alternatives.len() > MAX_ALTERNATIVES {
                    return Err(PatternError::TooMany);
                }
            }
        }
        Ok(Self { alternatives })
    }

    #[must_use]
    pub fn matches(&self, path: &str) -> bool {
        let segments: Vec<&str> = path.split('/').collect();
        self.alternatives
            .iter()
            .any(|pattern| match_segments(pattern, &segments))
    }
}

/// Splits on commas that are not inside braces.
fn split_top_level(source: &str) -> Result<Vec<&str>, PatternError> {
    let mut pieces = Vec::new();
    let mut depth = 0_u32;
    let mut start = 0;
    for (i, c) in source.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.checked_sub(1).ok_or(PatternError::Unbalanced)?,
            ',' if depth == 0 => {
                pieces.push(&source[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(PatternError::Unbalanced);
    }
    pieces.push(&source[start..]);
    Ok(pieces)
}

/// Expands the first brace group, recursively, into plain patterns.
fn expand(pattern: &str) -> Result<Vec<String>, PatternError> {
    let Some(open) = pattern.find('{') else {
        if pattern.contains('}') {
            return Err(PatternError::Unbalanced);
        }
        return Ok(vec![pattern.to_owned()]);
    };
    let mut depth = 0_u32;
    let mut close = None;
    let mut options = Vec::new();
    let mut option_start = open + 1;
    for (i, c) in pattern[open..].char_indices().map(|(i, c)| (i + open, c)) {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    options.push(&pattern[option_start..i]);
                    close = Some(i);
                    break;
                }
            }
            ',' if depth == 1 => {
                options.push(&pattern[option_start..i]);
                option_start = i + 1;
            }
            _ => {}
        }
    }
    let close = close.ok_or(PatternError::Unbalanced)?;
    let (head, tail) = (&pattern[..open], &pattern[close + 1..]);

    let mut out = Vec::new();
    for option in options {
        for rest in expand(&format!("{head}{option}{tail}"))? {
            out.push(rest);
            if out.len() > MAX_ALTERNATIVES {
                return Err(PatternError::TooMany);
            }
        }
    }
    Ok(out)
}

/// Whole-path matching, where `**` stands for any number of segments.
/// Dynamic programming over (pattern segment, path segment), so the cost is
/// bounded by their product rather than growing with each `**`.
fn match_segments(pattern: &[String], path: &[&str]) -> bool {
    // reachable[j]: the pattern so far can match exactly path[..j].
    let mut reachable = vec![false; path.len() + 1];
    reachable[0] = true;
    for segment in pattern {
        let mut next = vec![false; path.len() + 1];
        if segment == "**" {
            // Zero or more whole segments.
            let mut any = false;
            for j in 0..=path.len() {
                any |= reachable[j];
                next[j] = any;
            }
        } else {
            for j in 0..path.len() {
                if reachable[j] && match_one(segment.as_bytes(), path[j].as_bytes()) {
                    next[j + 1] = true;
                }
            }
        }
        reachable = next;
    }
    reachable[path.len()]
}

/// `*` and `?` within one segment, by the usual greedy scan with a single
/// backtrack point, which is linear in practice and never exponential.
fn match_one(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some(b'*') => {
                star = Some((p, t));
                p += 1;
            }
            Some(&c) if c == b'?' || c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match star {
                Some((sp, st)) => {
                    p = sp + 1;
                    t = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}
