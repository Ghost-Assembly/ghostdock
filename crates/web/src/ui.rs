//! Pieces every screen is built from.
//!
//! Each was once written out on every screen that needed it, and the copies
//! had begun to differ in small ways nobody chose. The markup and class
//! names are the stylesheet's contract, and the browser tests select by
//! them, so they are exactly what the copies produced.

use leptos::html::Pre;
use leptos::prelude::*;
use leptos_router::hooks::{use_params_map, use_query_map};

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

/// Where Back leads: the screen named by the `from` query, when a link
/// said where it came from, or else `default`.
///
/// Only a plain path within this app is followed. Anything else, such as
/// another site or a script, is ignored rather than trusted, since anyone
/// can write a link with any query.
#[must_use]
pub fn came_from(default: &'static str) -> Memo<String> {
    let query = use_query_map();
    Memo::new(move |_| {
        query
            .with(|q| q.get("from").filter(|path| is_local_path(path)))
            .unwrap_or_else(|| default.to_owned())
    })
}

/// A path on this site and nothing more: `/stacks/3`, never `//elsewhere`
/// or `javascript:`.
fn is_local_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'))
        && !path.contains("..")
}

/// One of the icons in `icons/lucide.svg`, drawn in the color of
/// the text around it. Hidden from assistive technology: it always sits
/// beside words that say the same thing.
#[component]
pub fn Icon(name: &'static str) -> impl IntoView {
    view! {
        <svg class="icon" aria-hidden="true" focusable="false">
            <use href=format!("/icons/lucide.svg#i-{name}") />
        </svg>
    }
}

/// What a stack runs, as its software's mark, or else its initials on a
/// plain tile. Both are drawn by the stylesheet in the ink color: a brand's
/// own colors would say something about state that is not so. Decorative,
/// since the stack's name is always beside it.
#[component]
pub fn StackIcon(#[prop(into)] name: String, icon: Option<String>) -> impl IntoView {
    // The server only names bundled icons; anything else is not made into
    // a path.
    let icon = icon.filter(|slug| is_slug(slug));
    let letters = icon.is_none().then(|| monogram(&name));
    let style = icon
        .as_ref()
        .map(|slug| format!("--brand:url(/brand-icons/{slug}.svg)"));
    view! {
        <span class="brand" aria-hidden="true" data-icon=icon data-letters=letters style=style></span>
    }
}

/// One or two letters for a name: the first of each of its first two
/// words.
fn monogram(name: &str) -> String {
    name.split(|c: char| !c.is_alphanumeric())
        .filter_map(|word| word.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect()
}

/// A bundled icon's name: lowercase letters and digits, nothing else.
fn is_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// The top of a screen: its name, and the way back to the screen before
/// or the one thing to do from here.
///
/// One component rather than the markup on every screen: every screen's
/// view then holds the same type here, and the compiler writes its code
/// once rather than once per screen.
#[component]
pub fn Topbar(
    #[prop(into)] title: Signal<String>,
    /// Where Back leads; no Back without it.
    #[prop(optional, into)]
    back: Option<Signal<String>>,
    /// Anything else at the end of the bar, such as a link to add something.
    #[prop(optional)]
    children: Option<Children>,
    /// Drawn before the title, inside the heading: a stack's icon.
    #[prop(optional, into)]
    lead: Option<ViewFn>,
) -> impl IntoView {
    view! {
        <header class="topbar">
            <h1 class="wordmark">{lead.map(|lead| move || lead.run())}{title}</h1>
            {back.map(|href| view! {
                <a class="topbar-link" href=href>
                    <Icon name="chevron-left" />
                    "Back"
                </a>
            })}
            {children.map(|children| children())}
        </header>
    }
}

/// A set of choices, one of them on: a row of buttons, the chosen one
/// pressed. By position, so every set of choices shares this one piece of
/// code whatever it chooses between.
#[component]
pub fn Choices(
    class: &'static str,
    label: &'static str,
    /// Each choice's words, and its icon if it has one.
    options: Vec<(&'static str, Option<&'static str>)>,
    #[prop(into)] chosen: Signal<usize>,
    on_choose: Callback<usize>,
) -> impl IntoView {
    view! {
        <div class=class role="group" aria-label=label>
            {options.into_iter().enumerate().map(|(i, (words, icon))| view! {
                <button class="button button-quiet" type="button"
                    aria-pressed=move || (chosen.get() == i).to_string()
                    on:click=move |_| on_choose.run(i)>
                    {icon.map(|name| view! { <Icon name /> })}
                    {words}
                </button>
            }).collect_view()}
        </div>
    }
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

/// A labelled line of text to type. One component for every such field, so
/// its code is in the bundle once.
#[component]
pub fn TextField(
    label: &'static str,
    value: RwSignal<String>,
    #[prop(optional)] placeholder: &'static str,
    /// Typed once and never shown again: a URL with a token in it, a token.
    #[prop(optional)]
    secret: bool,
) -> impl IntoView {
    view! {
        <Field label>
            <input
                class="field-input"
                type=if secret { "password" } else { "text" }
                autocomplete="off"
                autocapitalize="none"
                spellcheck="false"
                placeholder=placeholder
                prop:value=move || value.get()
                on:input=move |ev| value.set(event_target_value(&ev))
            />
        </Field>
    }
}

/// A labelled choice of one from `options`, each its value and its words.
#[component]
pub fn Pick(
    label: &'static str,
    #[prop(into)] options: Signal<Vec<(String, String)>>,
    value: RwSignal<String>,
) -> impl IntoView {
    view! {
        <Field label>
            <select class="field-input" on:change=move |ev| value.set(event_target_value(&ev))>
                // Each option says whether it is the chosen one: when the
                // list is read again its options are new, and a select left
                // to itself would fall back to the first.
                {move || options.get().into_iter().map(|(v, words)| {
                    let mine = v.clone();
                    view! { <option value=v selected=move || value.with(|c| *c == mine)>{words}</option> }
                }).collect_view()}
            </select>
        </Field>
    }
}

/// `(value, words)` pairs for [`Pick`], from literals.
#[must_use]
pub fn options(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(v, w)| ((*v).to_owned(), (*w).to_owned()))
        .collect()
}

/// A labelled yes or no.
#[component]
pub fn Tick(name: &'static str, detail: &'static str, value: RwSignal<bool>) -> impl IntoView {
    view! {
        <label class="check">
            <input type="checkbox" prop:checked=move || value.get() on:change=move |_| value.update(|v| *v = !*v) />
            <span class="check-name">{name}</span>
            <span class="check-detail">{detail}</span>
        </label>
    }
}

/// A secondary way in from a row: where, what it is called, and its icon.
pub type Aside = (String, &'static str, &'static str);

/// One row of a list: a bar, a name, and what else it says.
///
/// The bar's shape and color, and the icon the stylesheet puts before the
/// detail, give the row's state; a row with no state to give (`none`)
/// gets a plain, neutral bar, so color is never spent on decoration.
///
/// With an `href` the row is a link; without one it looks the same but
/// leads nowhere, which is how a row says there is nothing to open. The
/// children sit after the detail and before the count: figures, or an
/// action such as removing the thing the row stands for.
#[component]
pub fn Row(
    #[prop(into)] state: Signal<&'static str>,
    #[prop(into)] name: String,
    /// The name is an identifier (a container, a key, a path) rather than
    /// words, and is set in monospace.
    #[prop(optional)]
    ident: bool,
    /// Read once: a row does not change between link and not.
    #[prop(optional, into)]
    href: MaybeProp<String>,
    #[prop(optional, into)] detail: Option<String>,
    #[prop(optional, into)] count: Option<String>,
    #[prop(optional)] count_icon: Option<&'static str>,
    #[prop(optional)] asides: Vec<Aside>,
    /// A stack's icon before the name; the icon itself may be none, and the
    /// name's initials then stand in.
    #[prop(optional)]
    brand: Option<Option<String>>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let brand = brand.map(|icon| view! { <StackIcon name=name.clone() icon /> });
    let inner = view! {
        <span class="row-bar" data-state=state></span>
        <span class=if ident { "row-name row-id" } else { "row-name" }>{brand}{name}</span>
        {detail.map(|detail| view! {
            <span class="row-detail">{detail}</span>
        })}
        {children.map(|children| children())}
        {count.map(|count| view! {
            <span class="row-count">{count_icon.map(|name| view! { <Icon name /> })}{count}</span>
        })}
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
                .map(|(href, label, icon)| view! {
                    <a class="row-aside" href=href><Icon name=icon />{label}</a>
                })
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

/// One line of output in a log pane. A line from stderr is marked by a rule
/// beside it, not by color: it is a stream, not a fault, and plenty of
/// programs write their ordinary progress there.
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

    #[test]
    fn a_monogram_is_the_first_letters_of_the_first_two_words() {
        assert_eq!(monogram("nginx"), "N");
        assert_eq!(monogram("my-blog"), "MB");
        assert_eq!(monogram("Home media server"), "HM");
        assert_eq!(monogram("ghostdock_e2e_multilogs"), "GE");
        assert_eq!(monogram("Filler 12"), "F1");
        assert_eq!(monogram("--édition--"), "É");
        assert_eq!(monogram(" -- "), "");
    }

    #[test]
    fn only_a_plain_slug_becomes_a_file_path() {
        assert!(is_slug("nginx"));
        assert!(is_slug("zigbee2mqtt"));
        for bad in ["", "../x", "Nginx", "a b", "a/b", "x.svg", "a\"b"] {
            assert!(!is_slug(bad), "{bad}");
        }
    }

    #[test]
    fn back_follows_only_paths_on_this_site() {
        for ok in ["/", "/stacks/3", "/host", "/containers/web-1/resources"] {
            assert!(is_local_path(ok), "{ok}");
        }
        for bad in [
            "",
            "stacks/3",
            "//elsewhere.example",
            "/\\elsewhere",
            "javascript:alert(1)",
            "/stacks/3?x=<",
            "/../settings",
            "https://elsewhere.example/",
        ] {
            assert!(!is_local_path(bad), "{bad}");
        }
    }
}
