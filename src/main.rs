use tokio::sync::mpsc;
use tokio::time::sleep;
use std::time::Duration;

#[tokio::main]
async fn main() {
    // Create an asynchronous channel
    let (tx, mut rx) = mpsc::channel::<String>(1);

    // Spawn a Tokio task to fetch filenames asynchronously
    tokio::spawn(async move {
        fetch_filenames(tx).await;
    });

    // Handle filenames asynchronously
    while let Some(filename) = rx.recv().await {
        println!("Handling filename: {}", filename);
    }
}

async fn fetch_filenames(tx: mpsc::Sender<String>) {
    // Simulating a continuous process of fetching filenames
    for i in 0..10 {
        let filename = format!("file_{}.txt", i);
        tx.send(filename).await.expect("Failed to send filename");
        sleep(Duration::from_secs(1)).await;
    }
}