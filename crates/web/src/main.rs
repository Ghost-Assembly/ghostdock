//! GhostDock web client (Leptos, client-side rendered).
//!
//! ARCHITECTURAL INVARIANT: depends on `shared` and nothing else from
//! this workspace. All domain logic lives server-side, which is what
//! keeps a frontend rewrite confined to the view layer.
//!
//! Nothing here may panic. A release build aborts on panic, which stops the
//! whole client while leaving the page on screen: the purest way to freeze
//! the UI. The lints below make the usual sources a compile error; async
//! work goes through `screen::Screen` for the ones lints cannot see.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]

mod accounts;
mod activity;
mod api;
mod app;
mod charts;
mod cleanup;
mod confirm;
mod console;
mod deployment;
mod discover;
mod entry;
mod env;
mod events;
mod git_stack;
mod host;
mod load;
mod logs;
mod new_stack;
mod resources;
mod screen;
mod settings;
mod socket;
mod sources;
mod stack;
mod stacks;
mod status;
mod time;
mod tokens;
mod ui;
mod updates;

fn main() {
    set_panic_hook();
    keep_selections();
    leptos::mount::mount_to_body(app::App);
}

/// A mouse drag across a row does not open it.
///
/// Rows are links. Dragging across one to highlight its text used to open
/// whatever it pointed at, because a browser never starts a selection
/// inside a link and treats the release as a click. A click whose pointer
/// travelled more than a few pixels since going down, or that ends a
/// selection inside the row, was not a tap and is swallowed. Touch is left
/// alone: a finger that moves scrolls, and never clicks.
///
/// Registered on the window in the capture phase before the app mounts, so
/// it runs ahead of the router's own click handling and can stop it.
fn keep_selections() {
    use std::cell::Cell;
    use std::rc::Rc;
    use wasm_bindgen::JsCast as _;
    use wasm_bindgen::closure::Closure;

    const TAP_SLOP_PX: i32 = 6;

    let Some(window) = web_sys::window() else {
        return;
    };
    let down: Rc<Cell<Option<(i32, i32)>>> = Rc::new(Cell::new(None));

    let pressed = Rc::clone(&down);
    let on_down =
        Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |ev: web_sys::PointerEvent| {
            let mouse = ev.pointer_type() == "mouse";
            pressed.set(mouse.then(|| (ev.client_x(), ev.client_y())));
        });

    let on_click =
        Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |ev: web_sys::MouseEvent| {
            let Some(row) = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .and_then(|el| el.closest(".row-link").ok().flatten())
            else {
                return;
            };
            let dragged = down.take().is_some_and(|(x, y)| {
                (ev.client_x() - x).abs() > TAP_SLOP_PX || (ev.client_y() - y).abs() > TAP_SLOP_PX
            });
            let selecting = web_sys::window()
                .and_then(|w| w.get_selection().ok().flatten())
                .filter(|s| !s.is_collapsed())
                .and_then(|s| s.anchor_node())
                .is_some_and(|node| row.contains(Some(&node)));
            if dragged || selecting {
                ev.prevent_default();
                ev.stop_immediate_propagation();
            }
        });

    let _ = window.add_event_listener_with_callback_and_bool(
        "pointerdown",
        on_down.as_ref().unchecked_ref(),
        true,
    );
    let _ = window.add_event_listener_with_callback_and_bool(
        "click",
        on_click.as_ref().unchecked_ref(),
        true,
    );
    // Both live as long as the page.
    on_down.forget();
    on_click.forget();
}

/// Reports a panic and says so on screen.
///
/// A release build aborts on panic, so after this hook returns nothing in
/// the app will run again. Left alone the page would look alive and ignore
/// every tap. Instead the hook, which runs before the abort, covers the page
/// with a notice and a reload link written as plain HTML: nothing on it
/// depends on the app that just stopped. `rel="external"` keeps the router,
/// dead now, from intercepting the link.
///
/// Written out rather than taking `console_error_panic_hook`, which was last
/// published in 2021 and is ten lines of shim.
fn set_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&info.to_string().into());
        let Some(body) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.body())
        else {
            return;
        };
        let here = web_sys::window()
            .and_then(|w| w.location().href().ok())
            .unwrap_or_else(|| "/".to_owned());
        let _ = body.insert_adjacent_html(
            "beforeend",
            &format!(
                r#"<div class="crashed" role="alert">
                     <h1 class="entry-heading">Something went wrong</h1>
                     <p class="entry-note">GhostDock stopped responding on this page. Anything you
                       started on the server is still running.</p>
                     <a class="button" rel="external" href="{}">Reload</a>
                   </div>"#,
                here.replace('"', "%22")
            ),
        );
    }));
}
