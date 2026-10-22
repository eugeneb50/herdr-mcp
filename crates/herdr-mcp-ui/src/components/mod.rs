//! Routes + shared shell (header/nav) for the herdr-mcp web UI.

pub mod highlight;
pub mod home;
pub mod overview;
pub mod pane_options;
pub mod playground;
pub mod settings;
pub mod variables;

use dioxus::prelude::*;

use home::Home;
use overview::Overview;
use playground::Playground;
use settings::Settings;
use variables::Variables;

#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[layout(Shell)]
        #[route("/")]
        Home {},
        #[route("/overview")]
        Overview {},
        #[route("/playground")]
        Playground {},
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
                Link { class: "nav-link", to: Route::Overview {}, "Overview" }
                Link { class: "nav-link", to: Route::Playground {}, "Playground" }
                Link { class: "nav-link", to: Route::Variables {}, "Variables" }
                Link { class: "nav-link", to: Route::Settings {}, "Settings" }
            }
            a {
                class: "github-link",
                href: "https://github.com/eugeneb50/herdr-mcp",
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
