//! Routes + shared shell (header/nav) for the herdr-mcp web UI.
//!
//! Port order per spec: shell → docs → overview → trim → variables →
//! settings → playground. This step lands shell/home/docs/overview/trim;
//! variables/settings/playground routes arrive with their ports.

pub mod docs;
pub mod home;
pub mod overview;
pub mod playground;
pub mod settings;
pub mod trim;
pub mod variables;

use dioxus::prelude::*;

use docs::Docs;
use home::Home;
use overview::Overview;
use playground::Playground;
use settings::Settings;
use trim::Trim;
use variables::Variables;

#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[layout(Shell)]
        #[route("/")]
        Home {},
        #[route("/docs")]
        Docs {},
        #[route("/overview")]
        Overview {},
        #[route("/playground")]
        Playground {},
        #[route("/trim")]
        Trim {},
        #[route("/variables")]
        Variables {},
        #[route("/settings")]
        Settings {},
}

/// Shared page shell: sticky header + nav. Mirrors the React `Header`.
#[component]
fn Shell() -> Element {
    rsx! {
        header { class: "topbar",
            a { class: "brand", href: "/",
                span { class: "brand-mark", "◈" }
                span { class: "brand-name",
                    "herdr"
                    span { class: "brand-accent", "-mcp" }
                }
            }
            nav { class: "nav",
                Link { class: "nav-link", to: Route::Home {}, "Home" }
                Link { class: "nav-link", to: Route::Docs {}, "Docs" }
                Link { class: "nav-link", to: Route::Overview {}, "Overview" }
                Link { class: "nav-link", to: Route::Playground {}, "Playground" }
                Link { class: "nav-link", to: Route::Trim {}, "Trim" }
                Link { class: "nav-link", to: Route::Variables {}, "Variables" }
                Link { class: "nav-link", to: Route::Settings {}, "Settings" }
            }
            a {
                class: "github-link",
                href: "https://github.com",
                target: "_blank",
                rel: "noopener",
                "GitHub"
            }
        }
        main { class: "page",
            Outlet::<Route> {}
        }
    }
}
