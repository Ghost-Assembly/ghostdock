//! Reading several containers' output as one: which containers, and how
//! their lines are put in one order.

use shared::logs::TaggedLine;

/// Most containers one request may read or follow at once. Each one
/// followed holds a connection to the daemon open, and a phone showing the
/// output of a hundred containers is showing nobody anything.
pub const MAX_CONTAINERS: usize = 50;

/// Longest container name accepted. Docker's own names are far shorter;
/// this only bounds what a request can make the server hold.
const MAX_NAME: usize = 128;

/// Which containers to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Every running container on the host.
    All,
    /// Every container of one registered stack, running or not.
    Stack(i64),
    /// These, by name or id, whatever their state.
    Containers(Vec<String>),
}

impl Selection {
    /// From a request's `all`, `stack` and `containers` values, exactly one
    /// of which must be given. `all` is present with no value or `true`;
    /// `containers` is a comma-separated list of names or ids.
    pub fn parse(
        all: Option<&str>,
        stack: Option<&str>,
        containers: Option<&str>,
    ) -> Result<Self, String> {
        match (all, stack, containers) {
            (Some(all), None, None) => match all {
                "" | "true" | "1" => Ok(Self::All),
                _ => Err("all takes no value; leave it out to choose containers.".to_owned()),
            },
            (None, Some(stack), None) => stack
                .parse::<i64>()
                .map(Self::Stack)
                .map_err(|_| format!("stack is a stack's numeric id, not {stack}.")),
            (None, None, Some(list)) => {
                let mut names: Vec<String> = Vec::new();
                for name in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                    if !is_container_name(name) {
                        return Err(format!("Not a container name: {name}."));
                    }
                    if !names.iter().any(|n| n == name) {
                        names.push(name.to_owned());
                    }
                }
                if names.is_empty() {
                    return Err("Name at least one container.".to_owned());
                }
                check_count(names.len())?;
                Ok(Self::Containers(names))
            }
            (None, None, None) => {
                Err("Choose what to read: all, stack=<id> or containers=<names>.".to_owned())
            }
            _ => Err("Choose one of all, stack or containers, not several.".to_owned()),
        }
    }
}

/// Refuses more containers than [`MAX_CONTAINERS`], saying so.
pub fn check_count(count: usize) -> Result<(), String> {
    if count > MAX_CONTAINERS {
        Err(format!(
            "That is {count} containers. At most {MAX_CONTAINERS} can be read at once; choose fewer."
        ))
    } else {
        Ok(())
    }
}

/// A container name or id as Docker allows them: a letter or digit, then
/// letters, digits, `_`, `.` and `-`. Nothing that could end a path
/// segment or start another query value.
#[must_use]
pub fn is_container_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= MAX_NAME
        && chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Merges each container's lines into one list, oldest first, and keeps
/// the newest `limit` of them. The flag is true when lines were cut.
///
/// Each container's own order is kept as it is, and the lists are
/// interleaved by timestamp: the daemon copies stdout and stderr apart and
/// stamps each line as it reads it, so a log's order is not always its
/// timestamps' order, and sorting by stamp alone would reorder one
/// container's lines. A line without a stamp goes with the one before it.
/// Stamps are compared as times, not text: the daemon trims trailing zeros
/// from the fraction, so "…10Z" sorts after "…10.5Z" as text.
#[must_use]
pub fn merge(sources: Vec<Vec<TaggedLine>>, limit: usize) -> (Vec<TaggedLine>, bool) {
    let total: usize = sources.iter().map(Vec::len).sum();
    let mut heads: Vec<Head> = sources
        .into_iter()
        .map(|lines| Head {
            lines: lines.into_iter().peekable(),
            last: None,
        })
        .collect();

    let mut merged = Vec::with_capacity(total);
    loop {
        // The container whose next line is oldest; the first given wins a
        // tie. A plain scan: there are at most MAX_CONTAINERS of them.
        let mut next: Option<(usize, Option<i128>)> = None;
        for (i, head) in heads.iter_mut().enumerate() {
            let Some(key) = head.key() else { continue };
            if next.is_none_or(|(_, best)| key < best) {
                next = Some((i, key));
            }
        }
        let Some(line) = next
            .and_then(|(i, _)| heads.get_mut(i))
            .and_then(Head::take)
        else {
            break;
        };
        merged.push(line);
    }

    let cut = merged.len() > limit;
    if cut {
        merged.drain(..merged.len() - limit);
    }
    (merged, cut)
}

/// One container's remaining lines, and the time of the last one taken.
struct Head {
    lines: std::iter::Peekable<std::vec::IntoIter<TaggedLine>>,
    last: Option<i128>,
}

impl Head {
    /// When the next line was written, as nanoseconds; `None` inside means
    /// no time is known yet. `None` outside means there are no more lines.
    fn key(&mut self) -> Option<Option<i128>> {
        let line = self.lines.peek()?;
        Some(nanos(line).or(self.last))
    }

    fn take(&mut self) -> Option<TaggedLine> {
        let line = self.lines.next()?;
        if let Some(at) = nanos(&line) {
            self.last = Some(at);
        }
        Some(line)
    }
}

fn nanos(line: &TaggedLine) -> Option<i128> {
    let at = chrono::DateTime::parse_from_rfc3339(line.at.as_deref()?).ok()?;
    Some(i128::from(at.timestamp()) * 1_000_000_000 + i128::from(at.timestamp_subsec_nanos()))
}
