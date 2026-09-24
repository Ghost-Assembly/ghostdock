//! Registered stacks and their deployment history.

use chrono::{DateTime, Utc};
use shared::deployment::{
    Action, Deployment, DeploymentDetail, DeploymentStatus, GitSource, RegisteredStack, SourceKind,
    Trigger,
};

use crate::{Error, Result, Store};

/// Text written into the log of a deployment abandoned by a restart.
const INTERRUPTED: &str = "GhostDock restarted while this was running; \
                           the outcome was never recorded (marked interrupted)";

fn timestamp(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs, 0).unwrap_or_default()
}

/// id, host_id, slug, name, source_kind, repo_id, repo_url, git_ref,
/// compose_path, last_commit, created_at, updated_at
type StackTuple = (
    i64,
    i64,
    String,
    String,
    String,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    i64,
);

fn to_stack(row: StackTuple) -> RegisteredStack {
    let (
        id,
        host_id,
        slug,
        name,
        source_kind,
        repo_id,
        repo_url,
        git_ref,
        compose_path,
        last_commit,
        created_at,
        updated_at,
    ) = row;

    // The schema's CHECK guarantees a git stack has all three parts, so a
    // missing one here means the row was written outside this crate.
    let git = match (repo_id, repo_url, git_ref, compose_path) {
        (Some(repo_id), Some(repo_url), Some(git_ref), Some(compose_path)) => Some(GitSource {
            repo_id,
            repo_url,
            git_ref,
            compose_path,
            last_commit,
        }),
        _ => None,
    };

    RegisteredStack {
        id,
        host_id,
        slug,
        name,
        source_kind: if source_kind == "git" {
            SourceKind::Git
        } else {
            SourceKind::Inline
        },
        git,
        created_at: timestamp(created_at),
        updated_at: timestamp(updated_at),
    }
}

/// id, stack_id, action, trigger, status, exit_code, commit_sha,
/// started_at, finished_at
type DeploymentTuple = (
    i64,
    i64,
    String,
    String,
    String,
    Option<i64>,
    Option<String>,
    i64,
    Option<i64>,
);

fn to_deployment(
    (id, stack_id, action, trigger, status, exit_code, commit_sha, started_at, finished_at): DeploymentTuple,
) -> Deployment {
    Deployment {
        id,
        stack_id,
        action: match action.as_str() {
            "stop" => Action::Stop,
            "restart" => Action::Restart,
            "remove" => Action::Remove,
            _ => Action::Deploy,
        },
        trigger: match trigger.as_str() {
            "schedule" => Trigger::Schedule,
            _ => Trigger::Manual,
        },
        status: match status.as_str() {
            "succeeded" => DeploymentStatus::Succeeded,
            "failed" => DeploymentStatus::Failed,
            _ => DeploymentStatus::Running,
        },
        exit_code: exit_code.and_then(|c| i32::try_from(c).ok()),
        commit_sha,
        started_at: timestamp(started_at),
        finished_at: finished_at.map(timestamp),
    }
}

fn action_str(action: Action) -> &'static str {
    match action {
        Action::Deploy => "deploy",
        Action::Stop => "stop",
        Action::Restart => "restart",
        Action::Remove => "remove",
    }
}

fn trigger_str(trigger: Trigger) -> &'static str {
    match trigger {
        Trigger::Manual => "manual",
        Trigger::Schedule => "schedule",
    }
}

impl Store {
    /// Registers a stack. Fails with [`Error::SlugTaken`] on a duplicate.
    pub async fn stack_create(
        &self,
        host_id: i64,
        slug: &str,
        name: &str,
        compose_yaml: &str,
    ) -> Result<RegisteredStack> {
        let (id,) = sqlx::query_as::<_, (i64,)>(
            "INSERT INTO stacks (host_id, slug, name, source_kind, compose_yaml,
                                 created_at, updated_at)
             VALUES (?1, ?2, ?3, 'inline', ?4, unixepoch(), unixepoch())
             RETURNING id",
        )
        .bind(host_id)
        .bind(slug)
        .bind(name)
        .bind(compose_yaml)
        .fetch_one(self.pool())
        .await
        .map_err(|e| {
            if e.as_database_error()
                .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
            {
                Error::SlugTaken
            } else {
                Error::Sqlx(e)
            }
        })?;
        self.stack_by_id(id).await?.ok_or(Error::NotFound)
    }

    /// Registers a stack whose compose file lives in a repository.
    pub async fn stack_create_git(
        &self,
        host_id: i64,
        slug: &str,
        name: &str,
        repo_id: i64,
        git_ref: &str,
        compose_path: &str,
    ) -> Result<RegisteredStack> {
        let (id,) = sqlx::query_as::<_, (i64,)>(
            "INSERT INTO stacks (host_id, slug, name, source_kind, compose_yaml,
                                 repo_id, git_ref, compose_path, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'git', '', ?4, ?5, ?6, unixepoch(), unixepoch())
             RETURNING id",
        )
        .bind(host_id)
        .bind(slug)
        .bind(name)
        .bind(repo_id)
        .bind(git_ref)
        .bind(compose_path)
        .fetch_one(self.pool())
        .await
        .map_err(|e| {
            if e.as_database_error()
                .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
            {
                Error::SlugTaken
            } else {
                Error::Sqlx(e)
            }
        })?;
        self.stack_by_id(id).await?.ok_or(Error::NotFound)
    }

    /// Records the commit a stack was last deployed from.
    pub async fn stack_set_last_commit(&self, id: i64, commit: &str) -> Result<()> {
        sqlx::query("UPDATE stacks SET last_commit = ?2 WHERE id = ?1")
            .bind(id)
            .bind(commit)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn stacks_list(&self, host_id: i64) -> Result<Vec<RegisteredStack>> {
        let rows = sqlx::query_as::<_, StackTuple>(
            "SELECT s.id, s.host_id, s.slug, s.name, s.source_kind,
                    s.repo_id, r.url, s.git_ref, s.compose_path, s.last_commit,
                    s.created_at, s.updated_at
             FROM stacks s LEFT JOIN repos r ON r.id = s.repo_id
             WHERE s.host_id = ?1 ORDER BY s.slug",
        )
        .bind(host_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(to_stack).collect())
    }

    pub async fn stack_by_id(&self, id: i64) -> Result<Option<RegisteredStack>> {
        let row = sqlx::query_as::<_, StackTuple>(
            "SELECT s.id, s.host_id, s.slug, s.name, s.source_kind,
                    s.repo_id, r.url, s.git_ref, s.compose_path, s.last_commit,
                    s.created_at, s.updated_at
             FROM stacks s LEFT JOIN repos r ON r.id = s.repo_id
             WHERE s.id = ?1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(to_stack))
    }

    pub async fn stack_by_slug(&self, slug: &str) -> Result<Option<RegisteredStack>> {
        let row = sqlx::query_as::<_, StackTuple>(
            "SELECT s.id, s.host_id, s.slug, s.name, s.source_kind,
                    s.repo_id, r.url, s.git_ref, s.compose_path, s.last_commit,
                    s.created_at, s.updated_at
             FROM stacks s LEFT JOIN repos r ON r.id = s.repo_id
             WHERE s.slug = ?1",
        )
        .bind(slug)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(to_stack))
    }

    /// The stack's compose file, as stored.
    pub async fn stack_compose_yaml(&self, id: i64) -> Result<Option<String>> {
        let row = sqlx::query_as::<_, (String,)>("SELECT compose_yaml FROM stacks WHERE id = ?1")
            .bind(id)
            .fetch_optional(self.pool())
            .await?;
        Ok(row.map(|(yaml,)| yaml))
    }

    pub async fn stack_update_yaml(&self, id: i64, compose_yaml: &str) -> Result<()> {
        let result = sqlx::query(
            "UPDATE stacks SET compose_yaml = ?2, updated_at = unixepoch() WHERE id = ?1",
        )
        .bind(id)
        .bind(compose_yaml)
        .execute(self.pool())
        .await?;
        if result.rows_affected() == 0 {
            return Err(Error::NotFound);
        }
        Ok(())
    }

    /// Removes the registration. Deployment history goes with it.
    pub async fn stack_delete(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM stacks WHERE id = ?1")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Records the start of an attempt, before the command runs.
    pub async fn deployment_start(
        &self,
        stack_id: i64,
        action: Action,
        trigger: Trigger,
    ) -> Result<Deployment> {
        let row = sqlx::query_as::<_, DeploymentTuple>(
            "INSERT INTO deployments (stack_id, action, trigger, status, started_at)
             VALUES (?1, ?2, ?3, 'running', unixepoch())
             RETURNING id, stack_id, action, trigger, status, exit_code, commit_sha,
                       started_at, finished_at",
        )
        .bind(stack_id)
        .bind(action_str(action))
        .bind(trigger_str(trigger))
        .fetch_one(self.pool())
        .await?;
        Ok(to_deployment(row))
    }

    /// Records the outcome and the output compose produced.
    pub async fn deployment_finish(
        &self,
        id: i64,
        success: bool,
        exit_code: Option<i32>,
        log: &str,
        commit_sha: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE deployments
             SET status = ?2, exit_code = ?3, log = ?4, commit_sha = ?5,
                 finished_at = unixepoch()
             WHERE id = ?1",
        )
        .bind(id)
        .bind(if success { "succeeded" } else { "failed" })
        .bind(exit_code.map(i64::from))
        .bind(log)
        .bind(commit_sha)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Most recent attempts first.
    pub async fn deployments_for_stack(
        &self,
        stack_id: i64,
        limit: i64,
    ) -> Result<Vec<Deployment>> {
        let rows = sqlx::query_as::<_, DeploymentTuple>(
            "SELECT id, stack_id, action, trigger, status, exit_code, commit_sha,
                       started_at, finished_at
             FROM deployments WHERE stack_id = ?1 ORDER BY id DESC LIMIT ?2",
        )
        .bind(stack_id)
        .bind(limit)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(to_deployment).collect())
    }

    pub async fn deployment_detail(&self, id: i64) -> Result<Option<DeploymentDetail>> {
        let row = sqlx::query_as::<
            _,
            (
                i64,
                i64,
                String,
                String,
                String,
                Option<i64>,
                Option<String>,
                i64,
                Option<i64>,
                String,
            ),
        >(
            "SELECT id, stack_id, action, trigger, status, exit_code, commit_sha,
                    started_at, finished_at, log
             FROM deployments WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?;

        Ok(
            row.map(|(a, b, c, d, e, f, g, h, i, log)| DeploymentDetail {
                deployment: to_deployment((a, b, c, d, e, f, g, h, i)),
                log,
            }),
        )
    }

    /// Marks deployments left running by an unclean shutdown as failed.
    ///
    /// Run at startup: a row still marked running after a restart describes a
    /// command whose outcome nobody will ever learn, and leaving it in place
    /// would show a deploy spinning forever.
    pub async fn deployments_reap_stale(&self) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE deployments
             SET status = 'failed', finished_at = unixepoch(),
                 log = CASE WHEN log = '' THEN ?1 ELSE log || char(10) || ?1 END
             WHERE status = 'running'",
        )
        .bind(INTERRUPTED)
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected())
    }
}
