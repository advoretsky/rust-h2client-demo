use bytes::Bytes;
use async_channel::{Receiver, Sender};
use std::time::Duration;
use client::SendRequest;
use h2::client;
use reqwest::Url;
use tokio::net::{TcpStream};
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
                handle_filenames(receiver, i, "https://44.223.28.16:443/").await;
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
async fn new_connection(tcp: TcpStream) -> Result<SendRequest<Bytes>, h2::Error> {
    let (send_request, connection) = client::handshake(tcp).await?;
    tokio::spawn(async move {
        connection.await.unwrap();
    });
    Ok(send_request)
}

async fn handle_filenames(rx: Receiver<String>, id: usize, base_url: &str) {
    let send_request: Option<SendRequest<Bytes>> = Default::default();
    while let Ok(filename) = rx.recv().await {
        loop { // TODO implement limited number of retries here
            let send_request = send_request.clone();
            let mut send_request = match send_request {
                None => {
                    connect_to_server(base_url).await
                }
                Some(v) => v
            }
                .ready().await.unwrap();

            let mut uri = base_url.parse::<Url>().unwrap();
            uri.set_path(&filename);
            let request = http::Request::builder()
                .uri(uri.as_str())
                .body(())
                .unwrap();

            let (response, _) = send_request.send_request(request, true).unwrap();
            let (head, mut body) = response.await.unwrap().into_parts();
            println!("handler {:?} received response: {:?}", id, head); // TODO open a file
            let mut flow_control = body.flow_control().clone();
            while let Some(chunk) = body.data().await {
                let chunk = chunk.unwrap();
                println!("RX: {:?}", chunk); // TODO write to the file

                // Let the server send more data.
                let _ = flow_control.release_capacity(chunk.len());
            }
        }
    }
}

async fn connect_to_server(base_url: &str) -> SendRequest<Bytes> {
    let url = Url::parse(base_url).expect("Failed to parse URL");
    let host = url.host_str().expect("URL does not contain a host");
    let port = url.port().unwrap_or(443);

    loop {
        match TcpStream::connect(format!("{}:{}", host, port)).await {
            Ok(tcp) => match new_connection(tcp).await {
                Ok(connection) => return connection,
                Err(err) => {
                    eprintln!("Error establishing H2 connection: {}", err);
                }
            },
            Err(err) => {
                eprintln!("Error establishing TCP connection: {}", err);
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}