fn main() {
    // WebKit2GTK 4.1 + recent Mesa (Arch rolling, Fedora 40+, Ubuntu 24.04+)
    // crashes with SIGSEGV when the dmabuf renderer path is enabled. Opt out
    // before anything WebKit-related runs so the window comes up cleanly.
    // Users or distro packagers can still override by pre-setting the variable.
    //
    // Safety: this runs at program entry on the main thread, before any other
    // thread is spawned or any library reads the environment — the usual
    // `set_var` race doesn't apply here.
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        unsafe {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
    }

    linux_download_manager::run();
}
