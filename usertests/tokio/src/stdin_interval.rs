use std::io::{self, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::time::Duration;

use tokio::io::unix::AsyncFd;

#[derive(Debug)]
struct StdinFd;

impl AsRawFd for StdinFd {
    fn as_raw_fd(&self) -> RawFd {
        libc::STDIN_FILENO
    }
}

struct StdinMode {
    original_flags: i32,
    original_termios: libc::termios,
}

impl StdinMode {
    fn set_nonblocking_no_echo() -> io::Result<Self> {
        let fd = libc::STDIN_FILENO;

        let original_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if original_flags < 0 {
            return Err(io::Error::last_os_error());
        }

        let mut original_termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut original_termios) } < 0 {
            return Err(io::Error::last_os_error());
        }

        let mut termios = original_termios;
        termios.c_lflag &= !(libc::ECHO | libc::ICANON | libc::ISIG);
        termios.c_cc[libc::VMIN] = 1;
        termios.c_cc[libc::VTIME] = 0;

        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) } < 0 {
            return Err(io::Error::last_os_error());
        }

        if unsafe { libc::fcntl(fd, libc::F_SETFL, original_flags | libc::O_NONBLOCK) } < 0 {
            let err = io::Error::last_os_error();
            unsafe {
                libc::tcsetattr(fd, libc::TCSANOW, &original_termios);
            }
            return Err(err);
        }

        Ok(Self {
            original_flags,
            original_termios,
        })
    }
}

impl Drop for StdinMode {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.original_termios);
            libc::fcntl(libc::STDIN_FILENO, libc::F_SETFL, self.original_flags);
        }
    }
}

fn read_stdin(buf: &mut [u8]) -> io::Result<usize> {
    let ret = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr().cast(), buf.len()) };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(ret as usize)
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    let _mode = StdinMode::set_nonblocking_no_echo()?;
    let stdin = AsyncFd::new(StdinFd)?;
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut pending = Vec::new();
    let mut read_buf = [0; 64];

    println!("stdin interval test started");
    io::stdout().flush()?;

    loop {
        tokio::select! {
            result = stdin.readable() => {
                let mut guard = result?;
                match guard.try_io(|_| read_stdin(&mut read_buf)) {
                    Ok(Ok(0)) => break,
                    Ok(Ok(len)) => {
                        if let Some(pos) = read_buf[..len].iter().position(|byte| *byte == 3 || *byte == 4) {
                            pending.extend_from_slice(&read_buf[..pos]);
                            break;
                        }
                        pending.extend_from_slice(&read_buf[..len]);
                    }
                    Ok(Err(err)) => return Err(err),
                    Err(_would_block) => {}
                }
            }
            _ = interval.tick() => {
                let text = String::from_utf8_lossy(&pending);
                println!("tick: {text}");
                io::stdout().flush()?;
                pending.clear();
            }
        }
    }

    if !pending.is_empty() {
        let text = String::from_utf8_lossy(&pending);
        println!("stdin: {text}");
    }

    Ok(())
}
