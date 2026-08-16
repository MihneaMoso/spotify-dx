fn main() {
    // Read the client id from the build environment. Falls back to an empty
    // string so the project still compiles without credentials configured.
    let client_id = std::env::var("SPOTIFY_CLIENT_ID").unwrap_or_default();
    println!("cargo:rustc-env=SPOTIFY_CLIENT_ID={client_id}");
    println!("cargo:rerun-if-env-changed=SPOTIFY_CLIENT_ID");
}