//! Grouping containers into stacks.

use domain::stack::group;
use shared::container::{ComposeMembership, Container, ContainerState, Health};
use shared::stack::StackState;

fn container(name: &str, project: Option<&str>, state: ContainerState) -> Container {
    Container {
        id: format!("id-{name}"),
        name: name.to_owned(),
        image: "img:latest".to_owned(),
        state,
        status: String::new(),
        health: None,
        created: None,
        ports: Vec::new(),
        compose: project.map(|p| ComposeMembership {
            project: p.to_owned(),
            service: name.to_owned(),
        }),
    }
}

#[test]
fn empty_input_yields_nothing() {
    let g = group(Vec::new());
    assert!(g.stacks.is_empty() && g.unmanaged.is_empty());
}

#[test]
fn containers_are_grouped_by_project_and_sorted() {
    let g = group(vec![
        container("z", Some("beta"), ContainerState::Running),
        container("a", Some("alpha"), ContainerState::Running),
        container("b", Some("alpha"), ContainerState::Running),
    ]);

    let names: Vec<_> = g.stacks.iter().map(|s| s.project.as_str()).collect();
    assert_eq!(names, ["alpha", "beta"], "stacks sort by project name");

    let services: Vec<_> = g.stacks[0].containers.iter().map(|c| &c.name).collect();
    assert_eq!(services, ["a", "b"], "containers sort within a stack");
    assert_eq!(g.stacks[0].total_count, 2);
}

#[test]
fn containers_outside_compose_are_kept_separate() {
    let g = group(vec![
        container("managed", Some("proj"), ContainerState::Running),
        container("loose", None, ContainerState::Running),
    ]);

    assert_eq!(g.stacks.len(), 1);
    assert_eq!(g.unmanaged.len(), 1);
    assert_eq!(g.unmanaged[0].name, "loose");
}

#[test]
fn state_is_running_only_when_every_container_runs() {
    let g = group(vec![
        container("a", Some("p"), ContainerState::Running),
        container("b", Some("p"), ContainerState::Running),
    ]);
    assert_eq!(g.stacks[0].state, StackState::Running);
    assert_eq!(g.stacks[0].running_count, 2);
}

#[test]
fn state_is_stopped_when_nothing_runs() {
    let g = group(vec![
        container("a", Some("p"), ContainerState::Exited),
        container("b", Some("p"), ContainerState::Created),
    ]);
    assert_eq!(g.stacks[0].state, StackState::Stopped);
    assert_eq!(g.stacks[0].running_count, 0);
}

#[test]
fn state_is_degraded_when_only_some_run() {
    let g = group(vec![
        container("a", Some("p"), ContainerState::Running),
        container("b", Some("p"), ContainerState::Exited),
    ]);
    assert_eq!(g.stacks[0].state, StackState::Degraded);
    assert_eq!(g.stacks[0].running_count, 1);
}

#[test]
fn restarting_counts_as_running_for_the_stack() {
    let g = group(vec![container("a", Some("p"), ContainerState::Restarting)]);
    assert_eq!(g.stacks[0].state, StackState::Running);
}

#[test]
fn unhealthy_outranks_running() {
    let mut sick = container("a", Some("p"), ContainerState::Running);
    sick.health = Some(Health::Unhealthy);
    let g = group(vec![
        sick,
        container("b", Some("p"), ContainerState::Running),
    ]);

    assert_eq!(
        g.stacks[0].state,
        StackState::Unhealthy,
        "a stack whose containers all run but one is unhealthy is not healthy"
    );
}

#[test]
fn starting_health_does_not_mark_a_stack_unhealthy() {
    let mut warming = container("a", Some("p"), ContainerState::Running);
    warming.health = Some(Health::Starting);
    let g = group(vec![warming]);
    assert_eq!(g.stacks[0].state, StackState::Running);
}

#[test]
fn a_stopped_stack_is_stopped_even_with_stale_unhealthy_containers() {
    // Docker keeps reporting health=unhealthy on a container that has exited,
    // because the healthcheck can no longer pass. Verified against Docker
    // 29.7.2: a stopped container with a healthcheck reads
    // `exited health=unhealthy`. Health on a container that is not running is
    // stale, and treating it as live shows a deliberately stopped stack in
    // an alarm state.
    let mut a = container("a", Some("p"), ContainerState::Exited);
    a.health = Some(Health::Unhealthy);
    let mut b = container("b", Some("p"), ContainerState::Exited);
    b.health = Some(Health::Unhealthy);

    let g = group(vec![a, b]);
    assert_eq!(g.stacks[0].state, StackState::Stopped);
}

#[test]
fn stale_health_on_a_stopped_container_does_not_taint_a_degraded_stack() {
    let mut stopped = container("a", Some("p"), ContainerState::Exited);
    stopped.health = Some(Health::Unhealthy);
    let running = container("b", Some("p"), ContainerState::Running);

    let g = group(vec![stopped, running]);
    assert_eq!(
        g.stacks[0].state,
        StackState::Degraded,
        "only a running container's health is meaningful"
    );
}

mod merging {
    use super::container;
    use domain::stack::merge;
    use shared::container::ContainerState;
    use shared::deployment::SourceKind;
    use shared::stack::{Managed, StackState};

    fn managed(id: i64, name: &str) -> Managed {
        Managed {
            id,
            name: name.to_owned(),
            source_kind: SourceKind::Inline,
            busy: false,
            checks_down: Vec::new(),
        }
    }

    #[test]
    fn a_running_stack_is_matched_to_its_definition() {
        let g = merge(
            vec![container("a", Some("blog"), ContainerState::Running)],
            vec![("blog".to_owned(), managed(7, "My Blog"))],
        );

        assert_eq!(g.stacks.len(), 1);
        assert_eq!(g.stacks[0].state, StackState::Running);
        assert_eq!(g.stacks[0].managed.as_ref().unwrap().id, 7);
        assert_eq!(g.stacks[0].managed.as_ref().unwrap().name, "My Blog");
    }

    #[test]
    fn a_managed_stack_with_nothing_running_still_appears() {
        // "I deployed this and it is not running" is precisely what a user
        // needs to see; omitting it would make the stack silently vanish.
        let g = merge(Vec::new(), vec![("blog".to_owned(), managed(7, "My Blog"))]);

        assert_eq!(g.stacks.len(), 1);
        assert_eq!(g.stacks[0].project, "blog");
        assert_eq!(g.stacks[0].state, StackState::Empty);
        assert_eq!(g.stacks[0].total_count, 0);
        assert!(g.stacks[0].managed.is_some());
    }

    #[test]
    fn a_stack_ghostdock_does_not_manage_still_appears() {
        // The list should reflect the machine, not only the database.
        let g = merge(
            vec![container(
                "a",
                Some("someone-elses"),
                ContainerState::Running,
            )],
            Vec::new(),
        );

        assert_eq!(g.stacks.len(), 1);
        assert!(g.stacks[0].managed.is_none());
    }

    #[test]
    fn managed_and_unmanaged_stacks_share_one_sorted_list() {
        let g = merge(
            vec![
                container("a", Some("zebra"), ContainerState::Running),
                container("b", Some("apple"), ContainerState::Running),
            ],
            vec![
                ("apple".to_owned(), managed(1, "Apple")),
                ("mango".to_owned(), managed(2, "Mango")),
            ],
        );

        let projects: Vec<_> = g.stacks.iter().map(|s| s.project.as_str()).collect();
        assert_eq!(projects, ["apple", "mango", "zebra"]);
        assert!(g.stacks[0].managed.is_some(), "apple: managed and running");
        assert!(g.stacks[1].managed.is_some(), "mango: managed, not running");
        assert!(g.stacks[2].managed.is_none(), "zebra: running, not managed");
    }
}
