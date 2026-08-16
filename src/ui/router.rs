use dioxus::prelude::*;

use crate::ui::components::AppLayout;
use crate::ui::pages::{Album, Artist, ArtistTopTracks, Home, Library, Playlist, Search};

/// App routes. Nested layout wraps every page with the persistent shell.
#[derive(Clone, Routable, Debug, PartialEq)]
pub enum Route {
    #[layout(AppLayout)]
    #[route("/")]
    Home,

    #[route("/search")]
    Search,

    #[route("/library")]
    Library,

    #[route("/album/:id")]
    Album { id: String },

    #[route("/artist/:id")]
    Artist { id: String },

    #[route("/artist/:id/top")]
    ArtistTopTracks { id: String },

    #[route("/playlist/:id")]
    Playlist { id: String },
}