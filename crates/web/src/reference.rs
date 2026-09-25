//! The API and MCP reference, as the server generates it.

use leptos::prelude::*;
use shared::reference::{Endpoint, Reference};

use crate::api;
use crate::load::Load;
use crate::screen::Screen;
use crate::ui::Row;

#[component]
pub fn ApiReference() -> impl IntoView {
    let reference = RwSignal::new(Load::<Reference>::Loading);
    let screen = Screen::new();

    screen.load(async move {
        reference.set(Load::from(api::reference().await));
    });

    view! {
        <header class="topbar">
            <h1 class="wordmark">"API reference"</h1>
            <a class="topbar-link" href="/settings">"Back"</a>
        </header>

        {move || match reference.get() {
            Load::Loading => view! { <p class="state-note">"Loading"</p> }.into_any(),
            Load::Failed(message) => view! {
                <div class="state-note">
                    <p>"Could not read the reference."</p>
                    <p>{message}</p>
                </div>
            }
            .into_any(),
            Load::Ready(reference) => view! {
                <p class="verdict-count">
                    "Every endpoint and MCP tool, with what a token needs to use it."
                </p>
                {by_area(reference.endpoints)
                    .into_iter()
                    .map(|(area, endpoints)| view! {
                        <h2 class="group-heading">{area}</h2>
                        <ul class="rows">
                            {endpoints.into_iter().map(endpoint_row).collect_view()}
                        </ul>
                    })
                    .collect_view()}

                <h2 class="group-heading">"MCP tools"</h2>
                <ul class="rows">
                    {reference
                        .tools
                        .iter()
                        .map(|tool| view! {
                            <Row
                                state="stopped"
                                name=tool.name.clone()
                                detail=tool.description.clone()
                                count=tool.permission.as_str()
                            />
                        })
                        .collect_view()}
                </ul>
                <h2 class="group-heading">"MCP tool input"</h2>
                {reference
                    .tools
                    .into_iter()
                    .map(|tool| view! {
                        <details>
                            <summary>{tool.name}</summary>
                            <pre class="snippet" tabindex="0">{tool.input_schema}</pre>
                        </details>
                    })
                    .collect_view()}
            }
            .into_any(),
        }}
    }
}

/// A row for one endpoint. Grey: a reference has no state to colour.
fn endpoint_row(endpoint: Endpoint) -> impl IntoView {
    view! {
        <Row
            state="stopped"
            name=format!("{} {}", endpoint.method, endpoint.path)
            detail=endpoint.summary
            count=endpoint.access.label()
        />
    }
}

/// Endpoints under their areas, areas in the order the server lists them.
fn by_area(endpoints: Vec<Endpoint>) -> Vec<(String, Vec<Endpoint>)> {
    let mut areas: Vec<(String, Vec<Endpoint>)> = Vec::new();
    for endpoint in endpoints {
        match areas.iter_mut().find(|(area, _)| *area == endpoint.area) {
            Some((_, list)) => list.push(endpoint),
            None => areas.push((endpoint.area.clone(), vec![endpoint])),
        }
    }
    areas
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::reference::Access;

    fn endpoint(area: &str, path: &str) -> Endpoint {
        Endpoint {
            method: "GET".to_owned(),
            path: path.to_owned(),
            area: area.to_owned(),
            access: Access::Public,
            summary: String::new(),
        }
    }

    #[test]
    fn endpoints_are_grouped_in_the_order_their_areas_first_appear() {
        let grouped = by_area(vec![
            endpoint("Stacks", "/a"),
            endpoint("Hosts", "/b"),
            endpoint("Stacks", "/c"),
        ]);
        let shape: Vec<(String, Vec<String>)> = grouped
            .into_iter()
            .map(|(area, list)| (area, list.into_iter().map(|e| e.path).collect()))
            .collect();
        assert_eq!(
            shape,
            vec![
                ("Stacks".to_owned(), vec!["/a".to_owned(), "/c".to_owned()]),
                ("Hosts".to_owned(), vec!["/b".to_owned()]),
            ]
        );
    }
}
