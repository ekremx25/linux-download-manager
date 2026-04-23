fn main() {
    // WebKit2GTK 4.1 + recent Mesa (Arch rolling, Fedora 40+, Ubuntu 24.04+)
    // has two separate bugs:
    //   1. The dmabuf renderer path crashes with SIGSEGV at startup.
    //   2. With dmabuf off but compositing on, some Mesa drivers still
    //      render a blank window.
    // Turning both off lands on a safe software path that always paints.
    // Users or distro packagers can still override by pre-setting either
    // variable.
    //
    // Safety: program entry on the main thread, no other threads yet —
    // the usual `set_var` race doesn't apply.
    unsafe {
        if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
        if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
            std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        }
    }

    linux_download_manager::run();
}
