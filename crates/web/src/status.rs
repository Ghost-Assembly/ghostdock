//! How a state reads on screen: the key the stylesheet colours by, and the
//! words beside it. Kept in one place so two screens describing the same
//! thing cannot drift into saying it differently.

use shared::deployment::{Deployment, DeploymentStatus};
use shared::metrics::{format_bytes, format_cores};

/// How an operation went, as a row and as a verdict put it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The `data-state` of a row's bar.
    pub state: &'static str,
    /// The `data-tone` of a verdict line.
    pub tone: &'static str,
    /// Lower case, for a row; [`Outcome::heading`] for a verdict.
    pub word: String,
}

impl Outcome {
    #[must_use]
    pub fn of(deployment: &Deployment) -> Self {
        match deployment.status {
            DeploymentStatus::Succeeded => Self {
                state: "running",
                tone: "quiet",
                word: "succeeded".to_owned(),
            },
            DeploymentStatus::Running => Self {
                state: "degraded",
                tone: "degraded",
                word: "running now".to_owned(),
            },
            DeploymentStatus::Failed => Self {
                state: "unhealthy",
                tone: "bad",
                word: deployment
                    .exit_code
                    .map_or_else(|| "failed".to_owned(), |c| format!("failed (exit {c})")),
            },
        }
    }

    /// The word with a capital, to stand on its own.
    #[must_use]
    pub fn heading(&self) -> String {
        let mut chars = self.word.chars();
        chars.next().map_or_else(String::new, |first| {
            first.to_uppercase().chain(chars).collect()
        })
    }
}

/// "0.12 cores, 300 MiB", from whichever figures there are; `None` when
/// there are none.
#[must_use]
pub fn usage(cpu: Option<f64>, mem: Option<u64>) -> Option<String> {
    let parts: Vec<String> = [cpu.map(format_cores), mem.map(format_bytes)]
        .into_iter()
        .flatten()
        .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::deployment::{Action, Trigger};

    fn deployment(status: DeploymentStatus, exit_code: Option<i32>) -> Deployment {
        Deployment {
            id: 1,
            stack_id: 1,
            action: Action::Deploy,
            trigger: Trigger::Manual,
            status,
            exit_code,
            commit_sha: None,
            started_at: chrono::Utc::now(),
            finished_at: None,
        }
    }

    #[test]
    fn an_outcome_reads_the_same_in_a_row_and_a_verdict() {
        let ok = Outcome::of(&deployment(DeploymentStatus::Succeeded, Some(0)));
        assert_eq!(
            (ok.state, ok.tone, ok.word.as_str()),
            ("running", "quiet", "succeeded")
        );
        assert_eq!(ok.heading(), "Succeeded");

        let running = Outcome::of(&deployment(DeploymentStatus::Running, None));
        assert_eq!((running.state, running.tone), ("degraded", "degraded"));
        assert_eq!(running.heading(), "Running now");

        let failed = Outcome::of(&deployment(DeploymentStatus::Failed, Some(3)));
        assert_eq!((failed.state, failed.tone), ("unhealthy", "bad"));
        assert_eq!(failed.word, "failed (exit 3)");
        assert_eq!(
            Outcome::of(&deployment(DeploymentStatus::Failed, None)).word,
            "failed"
        );
    }

    #[test]
    fn usage_names_only_the_figures_there_are() {
        assert_eq!(
            usage(Some(0.12), Some(300 * 1024 * 1024)).as_deref(),
            Some("0.12 cores, 300 MiB")
        );
        assert_eq!(usage(None, Some(512)).as_deref(), Some("512 B"));
        assert_eq!(usage(Some(1.0), None).as_deref(), Some("1.00 cores"));
        assert_eq!(usage(None, None), None);
    }
}
