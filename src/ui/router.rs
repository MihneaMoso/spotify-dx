use dioxus::prelude::*;

use crate::ui::components::AppLayout;
use crate::ui::pages::{
    Album, Artist, Home, Library, Liked, Playlist, Queue, Search, Settings,
};

/// Router: authenticated pages. Nested layout wraps every page with the
/// persistent shell (top bar, rail, player bar).
#[derive(Clone, Routable, Debug, PartialEq)]
pub enum Route {
    #[layout(AppLayout)]
    #[route("/")]
    Home,

    #[route("/search")]
    Search,

    #[route("/library")]
    Library,

    #[route("/liked")]
    Liked,

    #[route("/queue")]
    Queue,

    #[route("/settings")]
    Settings,

    #[route("/album/:id")]
    Album { id: String },

    #[route("/artist/:id")]
    Artist { id: String },

    #[route("/playlist/:id")]
    Playlist { id: String },
}