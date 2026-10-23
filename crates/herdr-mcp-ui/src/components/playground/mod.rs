//! Playground: collections of saved requests (Postman style) and the live
//! herdr event stream.

pub mod builders;
pub mod collections;
pub mod events;
pub mod forms;
pub mod herdr_api;
pub mod recipes;
pub mod tools;

use dioxus::prelude::*;

use collections::Collections;
use events::HerdrEvents;

use super::pane_options::LivePaneOptions;

#[component]
pub fn Playground() -> Element {
    let mut tab: Signal<&str> = use_signal(|| "collections");

    rsx! {
        section { class: "page-head",
            div { class: "eyebrow", "Playground" }
            h1 { class: "page-title", "Collections" }
            p { class: "lede",
                "Save tool and herdr API calls as requests in collections. Send one, or run a collection in order; extract parts of a result into variables to chain the next request."
            }
        }
        LivePaneOptions {}
        div { class: "tabs",
            button {
                class: if *tab.read() == "collections" { "tab tab-active" } else { "tab" },
                onclick: move |_| tab.set("collections"),
                "Collections"
            }
            button {
                class: if *tab.read() == "events" { "tab tab-active" } else { "tab" },
                onclick: move |_| tab.set("events"),
                "herdr Events"
            }
        }
        if *tab.read() == "events" {
            HerdrEvents {}
        } else {
            Collections {}
        }
    }
}
