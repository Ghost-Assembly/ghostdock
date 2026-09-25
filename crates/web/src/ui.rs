//! Pieces every screen is built from.
//!
//! Each was once written out on every screen that needed it, and the copies
//! had begun to differ in small ways nobody chose. The markup and class
//! names are the stylesheet's contract, and the browser tests select by
//! them, so they are exactly what the copies produced.

use leptos::html::Pre;
use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

/// The numeric `:id` of the current route, following it as the route moves.
/// Zero, which names no record, when it is missing or not a number: the
/// read that follows is then refused like any other unknown id.
#[must_use]
pub fn route_id() -> Memo<i64> {
    let params = use_params_map();
    Memo::new(move |_| {
        params
            .with(|p| p.get("id").and_then(|v| v.parse::<i64>().ok()))
            .unwrap_or_default()
    })
}

/// Something that went wrong, said where it happened. Nothing at all while
/// there is nothing to say.
#[component]
pub fn ErrorNotice(#[prop(into)] error: Signal<Option<String>>) -> impl IntoView {
    move || {
        error
            .get()
            .map(|message| view! { <p class="notice" role="alert">{message}</p> })
    }
}

/// A labelled form control; the control is the child.
#[component]
pub fn Field(label: &'static str, children: Children) -> impl IntoView {
    view! {
        <label class="field">
            <span class="field-label">{label}</span>
            {children()}
        </label>
    }
}

/// One row of a list: a coloured bar, a name, and what else it says.
///
/// With an `href` the row is a link; without one it looks the same but
/// leads nowhere, which is how a row says there is nothing to open. The
/// children sit after the detail and before the count: figures, or an
/// action such as removing the thing the row stands for.
#[component]
pub fn Row(
    #[prop(into)] state: Signal<&'static str>,
    #[prop(into)] name: String,
    /// Read once: a row does not change between link and not.
    #[prop(optional, into)]
    href: MaybeProp<String>,
    #[prop(optional, into)] detail: Option<String>,
    #[prop(optional, into)] count: Option<String>,
    /// Secondary ways in, beside the row: where, and what it is called.
    #[prop(optional)]
    asides: Vec<(String, &'static str)>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let inner = view! {
        <span class="row-bar" data-state=move || state.get()></span>
        <span class="row-name">{name}</span>
        {detail.map(|detail| view! { <span class="row-detail">{detail}</span> })}
        {children.map(|children| children())}
        {count.map(|count| view! { <span class="row-count">{count}</span> })}
    };
    let body = match href.get_untracked() {
        Some(href) => view! { <a class="row-link" href=href>{inner}</a> }.into_any(),
        None => view! { <span class="row-link">{inner}</span> }.into_any(),
    };
    view! {
        <li class="row">
            {body}
            {asides
                .into_iter()
                .map(|(href, label)| view! { <a class="row-aside" href=href>{label}</a> })
                .collect_view()}
        </li>
    }
}

/// How close to the bottom of a log pane counts as reading the latest, in
/// pixels.
const PINNED_WITHIN: i32 = 48;

/// Whether the reader is at the bottom of `pane`, reading the latest. Only
/// then should a new line scroll it; someone who scrolled up to read stays
/// where they are.
pub fn pinned(pane: NodeRef<Pre>) -> bool {
    pane.get_untracked()
        .is_none_or(|el| el.scroll_top() + el.client_height() >= el.scroll_height() - PINNED_WITHIN)
}

/// Scrolls `pane` to its newest line.
pub fn to_bottom(pane: NodeRef<Pre>) {
    if let Some(el) = pane.get_untracked() {
        el.set_scroll_top(el.scroll_height());
    }
}

/// One line of output in a log pane, stderr marked as such.
#[component]
pub fn OutputLine(#[prop(into)] text: String, #[prop(optional)] stderr: bool) -> impl IntoView {
    let class = if stderr {
        "log-line log-stderr"
    } else {
        "log-line"
    };
    view! { <div class=class>{text}"\n"</div> }
}

/// Adds `item` to `list` if it is missing, and removes it if present: what
/// a checkbox does to the set it stands for.
pub fn toggle<T: PartialEq>(list: &mut Vec<T>, item: T) {
    match list.iter().position(|x| *x == item) {
        Some(i) => {
            list.remove(i);
        }
        None => list.push(item),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_adds_what_is_missing_and_removes_what_is_there() {
        let mut list = vec!["a", "b"];
        toggle(&mut list, "c");
        assert_eq!(list, ["a", "b", "c"]);
        toggle(&mut list, "a");
        assert_eq!(list, ["b", "c"]);
        toggle(&mut list, "a");
        assert_eq!(list, ["b", "c", "a"]);
    }
}
