use tokio::sync::{mpsc, Mutex};
use tokio::time::sleep;
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() {
    // Create an asynchronous channel
    let (tx, rx) = mpsc::channel::<String>(10);

    // Wrap the receiver in Arc<Mutex> to allow shared ownership
    let rx = Arc::new(Mutex::new(rx));

    // Spawn a Tokio task to fetch filenames asynchronously
    tokio::spawn(async move {
        fetch_filenames(tx).await;
    });

    // Number of handler tasks (change this value as needed)
    let num_handlers = 3;

    // Spawn multiple Tokio tasks for handling filenames concurrently
    let handler_tasks: Vec<_> = (0..num_handlers)
        .map(|i| {
            let rx = Arc::clone(&rx);
            tokio::spawn(async move {
                handle_filenames(&rx, i).await;
            })
        })
        .collect();

    // Wait for all handler tasks to finish
    for handler_task in handler_tasks {
        handler_task.await.expect("Handler task panicked");
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

async fn handle_filenames(rx: &Arc<Mutex<mpsc::Receiver<String>>>, id: usize) {
    // Simulating handling of filenames by multiple concurrent tasks
    while let Some(filename) = rx.lock().await.recv().await {
        println!("Handler {} is processing filename: {}", id, filename);
        // Simulate some processing time
        sleep(Duration::from_secs(2)).await;
    }
}
