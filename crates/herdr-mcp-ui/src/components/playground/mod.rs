//! Playground: tool runner + recipe builder tabs. Port of `Playground`.

pub mod events;
pub mod forms;
pub mod herdr_api;
pub mod recipes;
pub mod runner;

use dioxus::prelude::*;

use events::HerdrEvents;
use herdr_api::HerdrApi;
use recipes::{Loaded, RecipeBuilder, RecipeLibrary};
use runner::ToolRunner;

use super::pane_options::LivePaneOptions;

#[component]
pub fn Playground() -> Element {
    let mut tab: Signal<&str> = use_signal(|| "runner");
    let mut current: Signal<Loaded> = use_signal(Loaded::default);
    let mut refresh_key: Signal<u32> = use_signal(|| 0);
    // Bumped on every library pick: the builder is re-created from the pick.
    let mut loads: Signal<u32> = use_signal(|| 0);

    rsx! {
        section { class: "page-head",
            div { class: "eyebrow", "Playground" }
            h1 { class: "page-title", "Interactive tool console" }
            p { class: "lede",
                "Run individual tools or chain them into multi-step recipes. Requires the server with HTTP enabled."
            }
        }
        LivePaneOptions {}
        div { class: "tabs",
            button {
                class: if *tab.read() == "runner" { "tab tab-active" } else { "tab" },
                onclick: move |_| tab.set("runner"),
                "Tool Runner"
            }
            button {
                class: if *tab.read() == "herdr" { "tab tab-active" } else { "tab" },
                onclick: move |_| tab.set("herdr"),
                "herdr API"
            }
            button {
                class: if *tab.read() == "builder" { "tab tab-active" } else { "tab" },
                onclick: move |_| tab.set("builder"),
                "Recipe Builder"
            }
            button {
                class: if *tab.read() == "events" { "tab tab-active" } else { "tab" },
                onclick: move |_| tab.set("events"),
                "herdr Events"
            }
        }
        if *tab.read() == "runner" {
            ToolRunner {}
        } else if *tab.read() == "herdr" {
            HerdrApi {}
        } else if *tab.read() == "events" {
            HerdrEvents {}
        } else {
            div { class: "builder-grid",
                div {
                    RecipeLibrary {
                        refresh_key: *refresh_key.read(),
                        on_select: move |loaded: Loaded| {
                            current.set(loaded);
                            loads += 1;
                        },
                    }
                }
                div {
                    // Keys only re-create list children, so the builder sits
                    // in a one-item list keyed by the pick.
                    for load in std::iter::once(*loads.read()) {
                        RecipeBuilder {
                            key: "{load}",
                            initial: current.read().clone(),
                            on_saved: move |_| refresh_key += 1,
                        }
                    }
                }
            }
        }
    }
}
