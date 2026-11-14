//! Playground: collections (flows of requests and the nodes that route
//! them), their runs, and the live herdr event stream.

pub mod builders;
pub mod canvas;
pub mod collections;
pub mod events;
pub mod flow;
pub mod forms;
pub mod map;
pub mod recipes;
pub mod runs;

use dioxus::prelude::*;

use collections::Collections;
use events::HerdrEvents;
use map::Map;
use runs::Runs;

use super::pane_options::LivePaneOptions;

#[component]
pub fn Playground() -> Element {
    let mut tab: Signal<&str> = use_signal(|| "collections");

    rsx! {
        section { class: "page-head",
            div { class: "eyebrow", "Playground" }
            h1 { class: "page-title", "Collections" }
            p { class: "lede",
                "A collection is a flow: requests (tool and herdr API calls) wired to branches, loops, waits and other collections. Send one request, or run the flow and watch it; extract parts of a result into variables for the next node."
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
                class: if *tab.read() == "runs" { "tab tab-active" } else { "tab" },
                onclick: move |_| tab.set("runs"),
                "Runs"
            }
            button {
                class: if *tab.read() == "map" { "tab tab-active" } else { "tab" },
                onclick: move |_| tab.set("map"),
                "Map"
            }
            button {
                class: if *tab.read() == "events" { "tab tab-active" } else { "tab" },
                onclick: move |_| tab.set("events"),
                "herdr Events"
            }
        }
        match *tab.read() {
            "events" => rsx! { HerdrEvents {} },
            "runs" => rsx! { Runs {} },
            "map" => rsx! { Map {} },
            _ => rsx! { Collections {} },
        }
    }
}
