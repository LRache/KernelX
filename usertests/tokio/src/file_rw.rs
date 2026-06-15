use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TEST_PATH: &str = "tokio_file_rw.tmp";
const TEST_CONTENT: &str = "tokio file read write ok\n";

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    let _ = tokio::fs::remove_file(TEST_PATH).await;

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(TEST_PATH)
        .await?;

    file.write_all(TEST_CONTENT.as_bytes()).await?;
    file.sync_all().await?;
    drop(file);

    let mut file = tokio::fs::File::open(TEST_PATH).await?;
    let mut read_back = String::new();
    file.read_to_string(&mut read_back).await?;

    assert_eq!(read_back, TEST_CONTENT);
    tokio::fs::remove_file(TEST_PATH).await?;

    println!("tokio file read write ok");
    Ok(())
}
