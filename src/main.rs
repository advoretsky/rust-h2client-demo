mod h2connection;
mod tls;

use std::time::Duration;
use async_channel::{Receiver, Sender};
use futures::select_biased;
use http::Version;
use reqwest::Url;
use tokio::time::sleep;
use crate::h2connection::Connection;
use futures::FutureExt;

#[tokio::main]
async fn main() {
    // Create a broadcast channel
    let (tx, rx) = async_channel::bounded::<String>(1024);

    // Spawn a Tokio task to fetch filenames asynchronously
    tokio::spawn(async move {
        fetch_filenames(tx).await;
    });

    // Number of handler tasks (change this value as needed)
    let num_handlers = 1;

    // Spawn multiple Tokio tasks for handling filenames concurrently
    let handler_tasks: Vec<_> = (0..num_handlers)
        .map(|i| {
            let receiver = rx.clone();
            tokio::spawn(async move {
                handle_filenames(receiver, i, "https://filestorage.localdomain/").await;
            })
        })
        .collect();

    // Wait for all handler tasks to finish
    for handler_task in handler_tasks {
        handler_task.await.expect("Handler task panicked");
    }
}

const FILES_TO_GENERATE: i32 = 2000;
const BATCH_DELAY: u64 = 25;

async fn fetch_filenames(tx: Sender<String>) {
    // Simulating a continuous process of fetching filenames
    for i in 0..FILES_TO_GENERATE {
        let filename = format!("file{:02}.txt", i);
        match tx.send(filename).await {
            Ok(_) => {}
            Err(err) => {
                eprintln!("failed to dispatch filename: {}", err);
                break;
            }
        }
    }
}
async fn handle_filenames(rx: Receiver<String>, id: usize, base_url: &str) {
    let base_url = match base_url.parse::<Url>() {
        Ok(v) => v,
        Err(err) => {
            eprintln!("{}", err);
            return
        }
    };

    let mut handles = Vec::new();

    let mut h2: Option<Connection> = None;
    let mut receiver: Option<tokio::sync::mpsc::Receiver<bool>> = None;

    while let Ok(filename) = rx.recv().await {

        let mut connection = match h2.as_ref() {
            Some(v) => v,
            None => {
                let (h, r) = Connection::new(&base_url).await;
                h2 = Some(h);
                receiver = Some(r);
                h2.as_ref().unwrap()
            },
        }
            .clone();

        let uri = match base_url.join(&filename) {
            Ok(v) => v,
            Err(err) => {
                eprintln!("error building file path for {}: {}", filename, err);
                continue
            }
        };
        let request = match http::Request::builder()
            .version(Version::HTTP_2)
            .method("GET")
            .uri(uri.to_string())
            .body(()) {
            Ok(v) => v,
            Err(err) => {
                eprintln!("failed building a request: {}", err);
                continue
            }
        };

        loop {
            select_biased! {
                // it's important handling channel first
                _ = receiver.as_mut().unwrap().recv().fuse() => {
                    eprintln!("connection unhealthy signal received")
                }
                val = connection.clone().ready().fuse() => {
                    match val {
                        Err(err) => {
                            eprintln!("error waiting for SendRequest to become ready: {}", err);
                        }
                        Ok(_) => {
                            break
                        }
                    }
                }
            }
            println!("recreating a connection");
            receiver.unwrap().close();
            h2 = None;
            receiver = None;
        }

        let handle = tokio::spawn(async move {
            println!("sending request to fetch {}", uri.as_str());
            let (response, _) = connection.send_request(request.clone()).unwrap();

            let (head, mut body) = match response.await {
                Ok(r) => r.into_parts(),
                Err(err) => {
                    eprintln!("failed downloading file {}: {}", uri.as_str(), err);
                    connection.mark_connection_unhealthy().await;
                    return
                }
            };
            println!("handler {:?} received response: {:?}", id, head.status); // TODO open a file
            let mut flow_control = body.flow_control().clone();
            while let Some(chunk) = body.data().await {
                let chunk = chunk.unwrap();
                println!("RX: {:?} for {}", chunk.len(), filename); // TODO write to the file

                // Let the server send more data.
                let _ = flow_control.release_capacity(chunk.len());
            }
        });
        handles.push(handle);
        if handles.len() % 10 == 0 {
            println!("sleeping {} between batches", BATCH_DELAY);
            sleep(Duration::from_millis(BATCH_DELAY)).await;
        }

    }

    let total_number = handles.len();
    loop {
        sleep(Duration::from_millis(1000)).await;
        // Wait for a short period to check for completed tasks

        // Update the list of handles to remove completed tasks
        handles.retain(|handle| !handle.is_finished());

        // If there are still unfinished tasks, print how many are left
        if handles.is_empty() {
            println!("all {} tasks are finished", total_number);
            break;
        }
        println!("{} tasks not finished yet", handles.len());
    }
}

