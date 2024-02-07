use async_channel::{Receiver, Sender};
use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    // Create a broadcast channel
    let (tx, rx) = async_channel::bounded::<String>(1024);

    // Spawn a Tokio task to fetch filenames asynchronously
    tokio::spawn(async move {
        fetch_filenames(tx).await;
    });

    // Number of handler tasks (change this value as needed)
    let num_handlers = 3;

    // Spawn multiple Tokio tasks for handling filenames concurrently
    let handler_tasks: Vec<_> = (0..num_handlers)
        .map(|i| {
            let receiver = rx.clone();
            tokio::spawn(async move {
                handle_filenames(receiver, i).await;
            })
        })
        .collect();

    // Wait for all handler tasks to finish
    for handler_task in handler_tasks {
        handler_task.await.expect("Handler task panicked");
    }
}

async fn fetch_filenames(tx: Sender<String>) {
    // Simulating a continuous process of fetching filenames
    for i in 0..10 {
        let filename = format!("file_{}.txt", i);
        tx.send(filename).await.expect("Failed to send filename");
        sleep(Duration::from_secs(1)).await;
    }
}

async fn handle_filenames(rx: Receiver<String>, id: usize) {
    // Simulating handling of filenames by multiple concurrent tasks
    loop {
        match rx.recv().await {
            Ok(filename) => {
                println!("Handler {} is processing filename: {}", id, filename);
                // // Simulate some processing time
                // sleep(Duration::from_secs(2)).await;
            }
            Err(_) => {
                println!("Handler {} received channel closed signal", id);
                break; // Exit the loop gracefully when the channel is closed
            }
        }
    }
}
