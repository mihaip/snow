// JavaScript exports for Snow Emscripten frontend
// These functions are callable from Rust via extern "C"

mergeInto(LibraryManager.library, {
    js_did_open_video: function(width, height) {
        workerApi.didOpenVideo(width, height);
    },
    js_blit: function(bufPtr, bufSize) {
        workerApi.blit(bufPtr, bufSize);
    },
    js_disk_open: function(ptr) {
        return workerApi.disks.open(UTF8ToString(ptr));
    },
    js_disk_close: function(diskId) {
        workerApi.disks.close(diskId);
    },
    js_disk_size: function(diskId) {
        return workerApi.disks.size(diskId);
    },
    js_disk_read: function(diskId, bufPtr, offset, length) {
        return workerApi.disks.read(diskId, bufPtr, offset, length);
    },
    js_disk_write: function(diskId, bufPtr, offset, length) {
        return workerApi.disks.write(diskId, bufPtr, offset, length);
    },
    js_acquire_input_lock: function() {
        return workerApi.acquireInputLock();
    },
    js_release_input_lock: function() {
        workerApi.releaseInputLock();
    },
    js_has_mouse_position: function() {
        return workerApi.getInputValue(
            workerApi.InputBufferAddresses.mousePositionFlagAddr
        );
    },
    js_get_mouse_x_position: function() {
        return workerApi.getInputValue(
            workerApi.InputBufferAddresses.mousePositionXAddr
        );
    },
    js_get_mouse_y_position: function() {
        return workerApi.getInputValue(
            workerApi.InputBufferAddresses.mousePositionYAddr
        );
    },
    js_get_mouse_delta_x: function() {
        return workerApi.getInputValue(
            workerApi.InputBufferAddresses.mouseDeltaXAddr
        );
    },
    js_get_mouse_delta_y: function() {
        return workerApi.getInputValue(
            workerApi.InputBufferAddresses.mouseDeltaYAddr
        );
    },
    js_get_mouse_button_state: function() {
        return workerApi.getInputValue(
            workerApi.InputBufferAddresses.mouseButtonStateAddr
        );
    },
    js_console_log: function(ptr) {
        console.log(UTF8ToString(ptr));
    },
});
