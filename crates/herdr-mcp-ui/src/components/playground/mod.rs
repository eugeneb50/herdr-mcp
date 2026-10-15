//! Playground: tool runner + recipe builder tabs. Port of `Playground`.

pub mod forms;
pub mod recipes;
pub mod runner;

use dioxus::prelude::*;

use recipes::{RecipeBuilder, RecipeLibrary, Step};
use runner::ToolRunner;

#[component]
pub fn Playground() -> Element {
    let mut tab: Signal<&str> = use_signal(|| "runner");
    let mut current_name: Signal<String> = use_signal(String::new);
    let mut current_steps: Signal<Vec<Step>> = use_signal(Vec::new);
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
        div { class: "tabs",
            button {
                class: if *tab.read() == "runner" { "tab tab-active" } else { "tab" },
                onclick: move |_| tab.set("runner"),
                "Tool Runner"
            }
            button {
                class: if *tab.read() == "builder" { "tab tab-active" } else { "tab" },
                onclick: move |_| tab.set("builder"),
                "Recipe Builder"
            }
        }
        if *tab.read() == "runner" {
            ToolRunner {}
        } else {
            div { class: "builder-grid",
                div {
                    RecipeLibrary {
                        refresh_key: *refresh_key.read(),
                        on_select: move |(name, steps): (String, Vec<Step>)| {
                            current_name.set(name);
                            current_steps.set(steps);
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
                            initial_name: current_name.read().clone(),
                            initial_steps: current_steps.read().clone(),
                            on_saved: move |_| refresh_key += 1,
                        }
                    }
                }
            }
        }
    }
}
