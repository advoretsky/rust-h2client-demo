use std::time::Duration;
use async_channel::Receiver;
use futures::select_biased;
use http::Version;
use tokio::time::sleep;
use url::Url;
use crate::BATCH_DELAY;
use crate::h2connection::Connection;
use futures::FutureExt;

pub(crate) async fn handle_filenames(rx: Receiver<String>, id: u32, base_url: &str, max_streams: u32, insecure: bool) {
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
