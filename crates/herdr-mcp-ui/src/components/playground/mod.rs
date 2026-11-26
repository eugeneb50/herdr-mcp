//! Playground: collections (flows of requests and the nodes that route
//! them), their runs, and the live herdr event stream.

pub mod builders;
pub mod canvas;
pub mod collections;
pub mod events;
pub mod flow;
pub mod forms;
mod mail;
pub mod map;
mod queue_rows;
mod queues;
pub mod recipes;
pub mod runs;

use super::Route;
use dioxus::prelude::*;
use herdr_mcp_api::Uuid;

use collections::Collections;
use events::HerdrEvents;
use mail::Mail;
use map::Map;
use queues::Queues;
use runs::{LinkedRun, Runs};

use super::pane_options::LivePaneOptions;

pub(super) fn playground_tab(tab: &str) -> Route {
    Route::Playground {
        tab: (tab != "collections").then(|| tab.to_owned()),
        queue: None,
        item: None,
        run: None,
        mail_view: None,
        mail_to: None,
        mail_item: None,
    }
}

pub(super) fn run_link(id: Uuid) -> Route {
    Route::Playground {
        tab: Some("runs".into()),
        queue: None,
        item: None,
        run: Some(id.to_string()),
        mail_view: None,
        mail_to: None,
        mail_item: None,
    }
}

#[component]
pub fn Playground(
    tab: Option<String>,
    queue: Option<String>,
    item: Option<String>,
    run: Option<String>,
    mail_view: Option<String>,
    mail_to: Option<String>,
    mail_item: Option<String>,
) -> Element {
    let status = use_signal(String::new);
    let queue_key = format!("{queue:?}/{item:?}");
    let mail_key = format!("{mail_view:?}/{mail_to:?}/{mail_item:?}");
    let tab = tab.as_deref().unwrap_or("collections");
    let run = run.and_then(|id| id.parse::<Uuid>().ok());
    rsx! {

        section { class: "page-head",
            div { class: "eyebrow", "Playground" }
            h1 { class: "page-title",
                match tab {
                    "queues" => "Queues",
                    "mail" => "Mail",
                    "runs" => "Runs",
                    "map" => "Map",
                    "events" => "herdr Events",
                    _ => "Collections",
                }
            }
            if tab == "collections" {
                p { class: "lede",
                    "A collection is a flow: requests (tool and herdr API calls) wired to branches, loops, waits and other collections. Send one request, or run the flow and watch it; extract parts of a result into variables for the next node."
                }
            }
        }
        LivePaneOptions {}
        div { class: "tabs",
            for (id, title) in [("collections", "Collections"), ("runs", "Runs"), ("queues", "Queues"), ("mail", "Mail"), ("map", "Map"), ("events", "herdr Events")] {
                Link {
                    key: "{id}",
                    class: if tab == id { "tab tab-active" } else { "tab" },
                    to: playground_tab(id),
                    "{title}"
                }
            }
        }
        match tab {
            "events" => rsx! { HerdrEvents {} },
            "runs" => match run {
                Some(id) => rsx! { LinkedRun { key: "{id}", id } },
                None => rsx! { Runs {} },
            },
            "queues" => rsx! { Queues { key: "{queue_key}", queue, item, status } },
            "mail" => rsx! { Mail { key: "{mail_key}", to: mail_to, item: mail_item, status } },
            "map" => rsx! { Map {} },
            _ => rsx! { Collections {} },
        }
    }
}
