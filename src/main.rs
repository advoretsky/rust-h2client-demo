mod h2connection;
mod tls;
mod file_fetcher;
mod config;

use async_channel::Sender;

const BATCH_DELAY: u64 = 5;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let config = config::parse_args();


    // Create a broadcast channel
    let (tx, rx) = async_channel::bounded::<String>(1024);

    // Spawn a Tokio task to fetch filenames asynchronously
    tokio::spawn(async move {
        fetch_filenames(tx, config.files_number.unwrap()).await;
    });

    // Spawn multiple Tokio tasks for handling filenames concurrently
    let handler_tasks: Vec<_> = (0..config.concurrency)
        .map(|i| {
            let base_url = config.base_url.clone().unwrap();
            let receiver = rx.clone();
            tokio::spawn(async move {
                file_fetcher::handle_filenames(
                    receiver,
                    i,
                    base_url.as_str(),
                    config.max_streams,
                    config.insecure).await;
            })
        })
        .collect();

    // Wait for all handler tasks to finish
    for handler_task in handler_tasks {
        handler_task.await.expect("Handler task panicked");
    }
}

async fn fetch_filenames(tx: Sender<String>, files_number: u32) {
    // Simulating a continuous process of fetching filenames
    for i in 1..(files_number+1) {
        let filename = format!("file-{:06}.json", i);
        match tx.send(filename).await {
            Ok(_) => {}
            Err(err) => {
                eprintln!("failed to dispatch filename: {}", err);
                break;
            }
        }
    }
}

