mod h2connection;
mod tls;

use std::env;
use std::path::PathBuf;
use std::time::Duration;
use async_channel::{Receiver, Sender};
use futures::select_biased;
use http::Version;
use tokio::time::sleep;
use futures::FutureExt;
use url::Url;

use crate::h2connection::Connection;

const BATCH_DELAY: u64 = 5;

#[derive(Debug)]
struct Config {
    base_url: Option<String>,
    base_dir: Option<PathBuf>,
    files_number: Option<u32>,
    concurrency: u32,
    max_streams: u32,
    insecure: bool,
}

fn parse_args() -> Config {
    let args: Vec<String> = env::args().collect();

    let mut config = Config {
        base_url: None,
        base_dir: None,
        files_number: None,
        concurrency: 1,
        max_streams: 500,
        insecure: false,
    };

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-u" | "--base-url" => {
                i += 1;
                config.base_url = args.get(i).cloned();
            }
            "-d" | "--base-dir" => {
                i += 1;
                config.base_dir = args.get(i).map(PathBuf::from);
            }
            "-n" | "--files-number" => {
                i += 1;
                config.files_number = args.get(i).and_then(|val| val.parse().ok());
            }
            "-c" | "--concurrency" => {
                i += 1;
                if let Some(value) = args.get(i).and_then(|val| val.parse().ok()) {
                    config.concurrency = value
                }
            }
            "-s" | "--max-streams" => {
                i += 1;
                if let Some(value) = args.get(i).and_then(|val| val.parse().ok()) {
                    config.max_streams = value
                }
            }
            "-k" | "--insecure" => {
                config.insecure = true
            }

            _ => {
                eprintln!("Unknown option: {}", args[i]);
            }
        }
        i += 1;
    }

    config
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let config = parse_args();


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
                handle_filenames(
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
    for i in 0..files_number {
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
async fn handle_filenames(rx: Receiver<String>, id: u32, base_url: &str, max_streams: u32, insecure: bool) {
    let base_url = match base_url.parse::<Url>() {
        Ok(v) => v,
        Err(err) => {
            eprintln!("{}", err);
            return
        }
    };

    let mut handles = Vec::new();

    let mut h2connection: Option<Connection> = None;
    let mut receiver: Option<tokio::sync::mpsc::Receiver<bool>> = None;

    while let Ok(filename) = rx.recv().await {

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

        let mut connection = loop {
            let connection = match h2connection.as_ref() {
                Some(v) => v,
                None => {
                    let (c, r) = Connection::new(&base_url, insecure).await;
                    (h2connection, receiver) = (Some(c), Some(r));
                    h2connection.as_ref().unwrap()
                },
            }.clone();

            if max_streams == 0 || connection.requests_sent() < max_streams {
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
                                // the connection is healthy and ready
                                break connection
                            }
                        }
                    }
                }
            }
            // the connection has failed - drop it
            println!("closing a failed connection");
            receiver.unwrap().close();
            h2connection = None;
            receiver = None;
        };

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
            let mut flow_control = body.flow_control().clone();
            while let Some(chunk) = body.data().await {
                let chunk = chunk.unwrap();
                println!("handler {:?} status: {} RX: {:?} for {}", id, head.status, chunk.len(), filename); // TODO write to the file

                // Let the server send more data.
                let _ = flow_control.release_capacity(chunk.len());
            }
        });
        handles.push(handle);
        if handles.len() % 20 == 0 {
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

