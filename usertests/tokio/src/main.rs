use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::FromRawFd;

use tokio::io::unix::AsyncFd;

const MESSAGE: &[u8] = b"async-fd";

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(std::io::Error::last_os_error());
    }

    for fd in fds {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }

    let reader = unsafe { File::from_raw_fd(fds[0]) };
    let mut writer = unsafe { File::from_raw_fd(fds[1]) };

    let async_reader = AsyncFd::new(reader)?;
    let writer_task = tokio::spawn(async move {
        tokio::task::yield_now().await;
        writer.write_all(MESSAGE).expect("write to pipe");
    });

    let mut buffer = [0; MESSAGE.len()];

    loop {
        let mut guard = async_reader.readable().await?;
        match guard.try_io(|inner| {
            let mut reader = inner.get_ref();
            reader.read(&mut buffer)
        }) {
            Ok(result) => {
                let read_len = result?;
                assert_eq!(&buffer[..read_len], MESSAGE);
                break;
            }
            Err(_would_block) => continue,
        }
    }

    writer_task.await.expect("writer task panicked");
    println!("tokio AsyncFd await ok");
    Ok(())
}
