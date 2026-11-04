//! Browser-only pieces of sync.

use wasm_bindgen::JsCast;

/// Hands `bytes` to the browser as a download named `name`; the browser
/// asks where to save it or puts it in its downloads folder.
pub(crate) fn save_download(name: &str, bytes: &[u8]) -> Result<(), String> {
    let fail = |what: &str| format!("the browser could not save the file ({what})");
    let window = web_sys::window().ok_or_else(|| fail("no window"))?;
    let document = window.document().ok_or_else(|| fail("no document"))?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let blob = web_sys::Blob::new_with_u8_array_sequence(&parts).map_err(|_| fail("blob"))?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(|_| fail("url"))?;
    let link = document
        .create_element("a")
        .ok()
        .and_then(|element| element.dyn_into::<web_sys::HtmlAnchorElement>().ok())
        .ok_or_else(|| fail("link"))?;
    link.set_href(&url);
    link.set_download(name);
    link.click();
    // Revoking at once can cancel the download before it starts.
    matty_api::rt::spawn(async move {
        matty_api::rt::sleep(std::time::Duration::from_secs(60)).await;
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    Ok(())
}
