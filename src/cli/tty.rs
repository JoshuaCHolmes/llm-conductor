/// Watch /dev/tty for an Escape keypress while the model is streaming.
///
/// Returns a oneshot Receiver that fires when ESC is detected.
/// Caller must set `stop` to `true` when streaming finishes so the
/// watcher thread exits cleanly.
#[cfg(unix)]
pub fn spawn_esc_watcher(stop: std::sync::Arc<std::sync::atomic::AtomicBool>)
    -> tokio::sync::oneshot::Receiver<()>
{
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    std::thread::spawn(move || {
        let fd = unsafe {
            libc::open(
                b"/dev/tty\0".as_ptr() as *const libc::c_char,
                libc::O_RDONLY | libc::O_NONBLOCK,
            )
        };
        if fd < 0 { return; }

        // Save terminal state and enable single-character raw input
        let mut old_tio: libc::termios = unsafe { std::mem::zeroed() };
        unsafe { libc::tcgetattr(fd, &mut old_tio) };

        let mut raw_tio = old_tio;
        // Disable canonical (line-buffered) mode and echo; keep ISIG so Ctrl+C still works
        raw_tio.c_lflag &= !(libc::ECHO | libc::ICANON);
        raw_tio.c_cc[libc::VMIN] = 0;
        raw_tio.c_cc[libc::VTIME] = 0;
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw_tio) };

        while !stop.load(Ordering::Relaxed) {
            let mut buf = [0u8; 1];
            let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, 1) };
            if n == 1 && buf[0] == 0x1b {
                let _ = tx.send(());
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }

        unsafe {
            libc::tcsetattr(fd, libc::TCSANOW, &old_tio);
            libc::close(fd);
        }
    });

    rx
}

/// No-op stub for non-unix platforms.
#[cfg(not(unix))]
pub fn spawn_esc_watcher(_stop: std::sync::Arc<std::sync::atomic::AtomicBool>)
    -> tokio::sync::oneshot::Receiver<()>
{
    let (_tx, rx) = tokio::sync::oneshot::channel::<()>();
    rx
}
