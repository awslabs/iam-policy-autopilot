/**
 * Emscripten JS library providing browser fetch() to Rust via FFI.
 *
 * The Rust side calls `em_fetch_get_sync(url_ptr, url_len)` which suspends
 * the WASM execution (via JSPI) while the browser fetch completes.
 *
 * With JSPI, this async function acts as a suspending import — when called,
 * it suspends the WASM stack, awaits the fetch, then resumes with the result.
 */
addToLibrary({
  em_fetch_get_sync__async: true,
  em_fetch_get_sync: async function (urlPtr, urlLen) {
    const url = UTF8ToString(urlPtr, urlLen);
    try {
      const response = await fetch(url);
      if (!response.ok) {
        return 0; // null pointer signals error
      }
      const text = await response.text();
      const len = lengthBytesUTF8(text) + 1;
      const ptr = _malloc(len);
      stringToUTF8(text, ptr, len);
      return ptr;
    } catch (e) {
      console.error("em_fetch_get_sync failed:", e);
      return 0; // null pointer signals error
    }
  },

  em_fetch_free: function (ptr) {
    if (ptr) _free(ptr);
  },
});
