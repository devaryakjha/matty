//! The `matty-web` binary: Matty in a browser tab. `scripts/web build`
//! turns it into the page in `crates/matty-web/web/`.
//!
//! The page's query string takes `preview=1` (or `login`, `login-error`) for
//! the design fixtures `MATTY_PREVIEW` shows on the desktop, and
//! `backend=webgl` to skip WebGPU, and `demo=0` to start at the sign-in
//! card instead of the sample workspace in `demo/demo.sqlite` (made by
//! `scripts/demo/run`), which opens while no account is signed in. Off the web
//! this binary does nothing.

fn main() {
    #[cfg(target_family = "wasm")]
    {
        let params = web_sys::window()
            .and_then(|window| window.location().search().ok())
            .and_then(|search| web_sys::UrlSearchParams::new_with_str(&search).ok());
        let get = |key: &str| params.as_ref().and_then(|params| params.get(key));
        if let Some(preview) = get("preview") {
            matty_app::preview::set(preview);
        }
        let webgl = get("backend").as_deref() == Some("webgl");
        let demo: &'static [u8] = include_bytes!("../demo/demo.sqlite");
        let demo = (get("demo").as_deref() != Some("0")).then_some(demo);
        matty_app::run_web(
            include_bytes!("../assets/IBMPlexSans-Regular.ttf"),
            webgl,
            demo,
        );
    }
}
