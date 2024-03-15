mod tls;

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;
use bytes::Bytes;
use async_channel::{Receiver, Sender};
use client::SendRequest;
use h2::client;
use reqwest::Url;
use tokio::net::TcpStream;
use tokio::time::sleep;
use tokio_rustls::{rustls::ClientConfig, TlsConnector};
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;

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

async fn fetch_filenames(tx: Sender<String>) {
    // Simulating a continuous process of fetching filenames
    for i in 1..2001 {
        let filename = format!("file{:02}.txt", i);
        tx.send(filename).await.expect("Failed to send filename");
        // sleep(Duration::from_secs(1)).await;
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

    let h2 = connect_to_server(&base_url).await;
    'filename: while let Ok(filename) = rx.recv().await {

        let uri = match base_url.join(&filename) {
            Ok(v) => v,
            Err(err) => {
                eprintln!("error building file path for {}: {}", filename, err);
                continue
            }
        };
        let request = match http::Request::builder()
            .method("GET")
            .uri(uri.as_str())
            .body(()) {
            Ok(v) => v,
            Err(err) => {
                eprintln!("failed building a request: {}", err);
                continue
            }
        };

        if let Err(err) = h2.clone().ready().await {
            eprintln!("error waiting for SendRequest to become ready: {}", err);
            break 'filename
        }

        let mut h2 = h2.clone();

        let handle = tokio::spawn(async move {
            println!("sending request to fetch {}", uri.as_str());
            let (response, _) = h2.send_request(request.clone(), true).unwrap();
            let (head, mut body) = match response.await {
                Ok(r) => r.into_parts(),
                Err(err) => {
                    eprintln!("failed downloading file {}: {}", uri.as_str(), err);
                    return
                }
            };  // .unwrap().into_parts();
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

async fn connect_to_server(url: &Url) -> SendRequest<Bytes> {
    let host = url.host_str().expect("URL does not contain a host");
    let port = url.port().unwrap_or(443);

    loop {
        match attempt_connect(host, port).await {
            Ok(v) => return v,
            Err(err) => {
                eprintln!("{}", err);
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
        }
    }
}

async fn attempt_connect(host: &str, port: u16) -> Result<SendRequest<Bytes>, Box<dyn Error + Send>> {
    // // Load the system's root certificates
    // let mut root_store = RootCertStore::empty();
    // root_store.add_parsable_certificates(
    //     &rustls_native_certs::load_native_certs().expect("failed to load native certs")
    // );
    //
    // let mut config = ClientConfig::builder()
    //     .with_safe_defaults()
    //     // .with_custom_certificate_verifier(Arc::new(tls::NoCertificateVerification{}))
    //     .with_root_certificates(root_store)
    //     .with_no_client_auth();

    let mut config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(tls::SkipServerVerification{}))
        .with_no_client_auth();

    config.alpn_protocols.push(b"h2".to_vec()); // Enable HTTP/2 ALPN protocol negotiation.
    let tls_connector = TlsConnector::from(Arc::new(config));

    let authority = format!("{}:{}", host, port);
    let tcp_stream = match TcpStream::connect(&authority).await {
        Ok(v) => v,
        Err(err) => return Err(Box::new(err))
    };
    let host_arc: String = host.to_owned();
    let tls_stream = match tls_connector
        .connect(
            ServerName::try_from(host_arc)
                .expect(&format!("invalid host name: {}", host)),
            tcp_stream
        ).await {
        Ok(v) => v,
        Err(err) => return Err(Box::new(err))
    };
    return match negotiate_h2_connection(tls_stream).await {
        Ok(v) => Ok(v),
        Err(err) => Err(Box::new(err))
    };
}

async fn negotiate_h2_connection(tcp: TlsStream<TcpStream>) -> Result<SendRequest<Bytes>, h2::Error> {
    let (send_request, connection) = client::handshake(tcp).await?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            eprintln!("Error occurred while waiting for connection to close: {}", err);
        }
    });
    Ok(send_request)
}
