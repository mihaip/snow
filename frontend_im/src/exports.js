// JavaScript exports for Snow Emscripten frontend
// These functions are callable from Rust via extern "C"

mergeInto(LibraryManager.library, {
    js_did_open_video: function(width, height) {
        workerApi.didOpenVideo(width, height);
    },
    js_blit: function(bufPtr, bufSize) {
        workerApi.blit(bufPtr, bufSize);
    },
    js_console_log: function(ptr) {
        console.log(UTF8ToString(ptr));
    },
});
