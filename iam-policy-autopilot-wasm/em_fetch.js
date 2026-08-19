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
      // lengthBytesUTF8 returns byte length *excluding* the null terminator,
      // so +1 gives us space for the null byte that stringToUTF8 appends.
      const len = lengthBytesUTF8(text) + 1;
      const ptr = _malloc(len);
      stringToUTF8(text, ptr, len);
      return ptr;
    } catch (e) {
      return 0; // null pointer signals error to Rust
    }
  },

  em_fetch_free: function (ptr) {
    if (ptr) _free(ptr);
  },
});
