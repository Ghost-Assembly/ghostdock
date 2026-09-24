//! Stack registration and deployment history.

use shared::deployment::{Action, DeploymentStatus, SourceKind, Trigger};
use store::Store;
use store::hosts::LOCAL_HOST_ID;

const YAML: &str = "services:\n  web:\n    image: nginx\n";

async fn store() -> Store {
    Store::open_in_memory().await.expect("store")
}

async fn a_stack(s: &Store, slug: &str) -> i64 {
    s.stack_create(LOCAL_HOST_ID, slug, slug, YAML)
        .await
        .expect("create")
        .id
}

#[tokio::test]
async fn register_then_read_back() {
    let s = store().await;
    let created = s
        .stack_create(LOCAL_HOST_ID, "blog", "My Blog", YAML)
        .await
        .unwrap();

    assert_eq!(created.slug, "blog");
    assert_eq!(created.name, "My Blog");
    assert_eq!(created.source_kind, SourceKind::Inline);
    assert_eq!(created.host_id, LOCAL_HOST_ID);

    assert_eq!(
        s.stack_by_id(created.id).await.unwrap(),
        Some(created.clone())
    );
    assert_eq!(s.stack_by_slug("blog").await.unwrap(), Some(created));
    assert_eq!(
        s.stack_compose_yaml(1).await.unwrap().as_deref(),
        Some(YAML)
    );
}

#[tokio::test]
async fn slugs_are_unique_because_they_are_project_names() {
    let s = store().await;
    a_stack(&s, "blog").await;

    assert!(
        matches!(
            s.stack_create(LOCAL_HOST_ID, "blog", "Other", YAML).await,
            Err(store::Error::SlugTaken)
        ),
        "two stacks sharing a project name would fight over the same containers"
    );
}

#[tokio::test]
async fn listing_is_scoped_to_a_host_and_ordered_by_name() {
    let s = store().await;
    s.stack_create(LOCAL_HOST_ID, "zebra", "Zebra", YAML)
        .await
        .unwrap();
    s.stack_create(LOCAL_HOST_ID, "apple", "Apple", YAML)
        .await
        .unwrap();

    let names: Vec<_> = s
        .stacks_list(LOCAL_HOST_ID)
        .await
        .unwrap()
        .into_iter()
        .map(|st| st.slug)
        .collect();
    assert_eq!(names, ["apple", "zebra"]);
    assert!(s.stacks_list(999).await.unwrap().is_empty());
}

#[tokio::test]
async fn updating_yaml_bumps_updated_at() {
    let s = store().await;
    let id = a_stack(&s, "blog").await;
    let before = s.stack_by_id(id).await.unwrap().unwrap();

    s.stack_update_yaml(id, "services: {}\n").await.unwrap();

    let after = s.stack_by_id(id).await.unwrap().unwrap();
    assert_eq!(
        s.stack_compose_yaml(id).await.unwrap().as_deref(),
        Some("services: {}\n")
    );
    assert!(after.updated_at >= before.updated_at);
}

#[tokio::test]
async fn deleting_a_stack_takes_its_history_with_it() {
    let s = store().await;
    let id = a_stack(&s, "blog").await;
    s.deployment_start(id, Action::Deploy, Trigger::Manual)
        .await
        .unwrap();

    s.stack_delete(id).await.unwrap();

    assert_eq!(s.stack_by_id(id).await.unwrap(), None);
    assert!(
        s.deployments_for_stack(id, 10).await.unwrap().is_empty(),
        "history must not outlive the stack it describes"
    );
}

#[tokio::test]
async fn a_deployment_is_recorded_before_it_runs() {
    let s = store().await;
    let id = a_stack(&s, "blog").await;

    let d = s
        .deployment_start(id, Action::Deploy, Trigger::Manual)
        .await
        .unwrap();

    assert_eq!(d.status, DeploymentStatus::Running);
    assert_eq!(d.action, Action::Deploy);
    assert_eq!(d.trigger, Trigger::Manual);
    assert_eq!(d.finished_at, None);
    assert_eq!(d.exit_code, None);
}

#[tokio::test]
async fn finishing_records_the_outcome_and_the_real_output() {
    let s = store().await;
    let id = a_stack(&s, "blog").await;
    let d = s
        .deployment_start(id, Action::Deploy, Trigger::Manual)
        .await
        .unwrap();

    s.deployment_finish(d.id, false, Some(1), "error: no such image\n", None)
        .await
        .unwrap();

    let detail = s.deployment_detail(d.id).await.unwrap().unwrap();
    assert_eq!(detail.deployment.status, DeploymentStatus::Failed);
    assert_eq!(detail.deployment.exit_code, Some(1));
    assert!(detail.deployment.finished_at.is_some());
    assert_eq!(
        detail.log, "error: no such image\n",
        "compose's own words are what the user needs to see"
    );
}

#[tokio::test]
async fn history_is_newest_first_and_limited() {
    let s = store().await;
    let id = a_stack(&s, "blog").await;
    for _ in 0..3 {
        s.deployment_start(id, Action::Deploy, Trigger::Manual)
            .await
            .unwrap();
    }

    let recent = s.deployments_for_stack(id, 2).await.unwrap();
    assert_eq!(recent.len(), 2);
    assert!(recent[0].id > recent[1].id, "newest first");
}

#[tokio::test]
async fn stale_running_deployments_are_reaped_at_startup() {
    let s = store().await;
    let id = a_stack(&s, "blog").await;
    let orphan = s
        .deployment_start(id, Action::Deploy, Trigger::Manual)
        .await
        .unwrap();
    let finished = s
        .deployment_start(id, Action::Stop, Trigger::Manual)
        .await
        .unwrap();
    s.deployment_finish(finished.id, true, Some(0), "ok", None)
        .await
        .unwrap();

    let reaped = s.deployments_reap_stale().await.unwrap();

    assert_eq!(reaped, 1, "only the one still marked running");
    let detail = s.deployment_detail(orphan.id).await.unwrap().unwrap();
    assert_eq!(
        detail.deployment.status,
        DeploymentStatus::Failed,
        "a deploy interrupted by a restart must not spin forever"
    );
    assert!(
        detail.log.contains("interrupted"),
        "and should say why: {}",
        detail.log
    );
}
