use std::time::Duration;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let ticker = tokio::spawn(async {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            println!("tick");
        }
    });

    ticker.await.expect("tick coroutine panicked");
}
