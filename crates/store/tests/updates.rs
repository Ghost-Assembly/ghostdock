//! What is waiting to be applied.

use shared::update::ImageStatus;
use store::Store;
use store::hosts::LOCAL_HOST_ID;

const YAML: &str = "services:\n  web:\n    image: nginx\n";

fn image(name: &str, running: &str, available: Option<&str>) -> ImageStatus {
    ImageStatus {
        image: name.to_owned(),
        running: Some(running.to_owned()),
        available: available.map(str::to_owned),
        error: None,
    }
}

#[tokio::test]
async fn every_stack_on_a_host_is_read_at_once_as_each_is_alone() {
    let s = Store::open_in_memory().await.unwrap();
    let checked = s
        .stack_create(LOCAL_HOST_ID, "blog", "Blog", YAML)
        .await
        .unwrap()
        .id;
    let never = s
        .stack_create(LOCAL_HOST_ID, "wiki", "Wiki", YAML)
        .await
        .unwrap()
        .id;
    let failed = s
        .stack_create(LOCAL_HOST_ID, "shop", "Shop", YAML)
        .await
        .unwrap()
        .id;
    s.update_record(
        checked,
        Some("abc"),
        None,
        &[
            image("nginx", "sha256:1", Some("sha256:2")),
            image("alpine", "sha256:3", Some("sha256:3")),
        ],
    )
    .await
    .unwrap();
    s.stack_set_last_commit(checked, "def").await.unwrap();
    s.stack_set_auto_apply(checked, true).await.unwrap();
    s.update_record(failed, None, Some("unreachable"), &[])
        .await
        .unwrap();

    let all = s.update_statuses(LOCAL_HOST_ID).await.unwrap();

    assert_eq!(all.len(), 3);
    for id in [checked, never, failed] {
        let (status, auto_apply) = all.get(&id).cloned().unwrap();
        assert_eq!(status, s.update_status(id).await.unwrap(), "stack {id}");
        assert_eq!(auto_apply, s.stack_auto_apply(id).await.unwrap());
    }
    let (blog, auto) = &all[&checked];
    assert!(*auto);
    assert_eq!(blog.deployed_commit.as_deref(), Some("def"));
    let names: Vec<_> = blog.images.iter().map(|i| i.image.as_str()).collect();
    assert_eq!(names, ["alpine", "nginx"], "sorted by image");
    assert!(all[&never].0.checked_at.is_none());
    assert_eq!(all[&failed].0.error.as_deref(), Some("unreachable"));
}

#[tokio::test]
async fn another_hosts_stacks_are_not_included() {
    let s = Store::open_in_memory().await.unwrap();
    s.stack_create(LOCAL_HOST_ID, "blog", "Blog", YAML)
        .await
        .unwrap();

    assert!(
        s.update_statuses(LOCAL_HOST_ID + 1)
            .await
            .unwrap()
            .is_empty()
    );
}
