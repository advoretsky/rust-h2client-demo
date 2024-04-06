use std::fmt;
use std::fmt::Display;
use std::time::Duration;
use async_channel::Receiver;
use futures::select_biased;
use http::Version;
use tokio::time::sleep;
use url::Url;
use crate::BATCH_DELAY;
use crate::h2connection::Connection;
use futures::FutureExt;
use tokio::select;
use tokio::sync::mpsc;

#[derive(Clone)]
struct Task {
    filename: String,
    retry_sender: mpsc::Sender<Task>,
}

impl Display for Task {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.filename)
    }
}

pub(crate) async fn handle_filenames(rx: Receiver<String>, id: u32, base_url: &str, max_streams: u32, insecure: bool) {
    let base_url = match base_url.parse::<Url>() {
        Ok(v) => v,
        Err(err) => {
            eprintln!("{}", err);
            return
        }
    };

    let (input_sender, input_receiver) = mpsc::channel::<Task>(256);
    let (retry_sender, retry_receiver) = mpsc::channel::<Task>(1024);
    let mut combined_receiver = join_mpsc(retry_receiver, input_receiver);

    let feeder = tokio::spawn(async move {
        while let Ok(filename) = rx.recv().await {
            let task = Task {
                filename,
                retry_sender: retry_sender.clone(),
            };
            match input_sender.send(task).await {
                Ok(_) => {}
                Err(err) => {
                    eprintln!("error feeding filename: {}", err);
                    break
                }
            }
        }
    });

    let mut h2connection: Option<Connection> = None;
    let mut receiver: Option<mpsc::Receiver<bool>> = None;

    let mut handles = Vec::new();
    let mut streams_used: usize = 0;
    let mut connection_established: usize = 0;

    while let Some(task) = combined_receiver.recv().await {

        let mut connection = loop {
            let connection = match h2connection.as_ref() {
                Some(v) => v,
                None => {
                    let (c, r) = Connection::new(&base_url, insecure).await;
                    connection_established += 1;
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
                                // the connection is healthy and ready - return it
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

        let uri = match base_url.join(&task.filename) {
            Ok(v) => v,
            Err(err) => {
                eprintln!("error building file path for {}: {}. skipping", task.filename, err);
                return
            }
        };
        let request = match http::Request::builder()
            .version(Version::HTTP_2)
            .method("GET")
            .uri(uri.to_string())
            .body(()) {
            Ok(v) => v,
            Err(err) => {
                eprintln!("failed building a request: {}. skipping", err);
                return
            }
        };

        let handle = tokio::spawn(async move {

            println!("sending request to fetch {}", uri.as_str());
            let (response, _) = connection.send_request(request.clone()).unwrap();

            let (head, mut body) = match response.await {
                Ok(r) => r.into_parts(),
                Err(err) => {
                    eprintln!("failed downloading file {}: {}", uri.as_str(), err);
                    connection.mark_connection_unhealthy().await;
                    match task.retry_sender.send(task.clone()).await {
                        Ok(_) => {
                            println!("successfully sent {} to the retry channel", task.filename)
                        }
                        Err(err) => {
                            eprintln!("error sending {} to the retry channel: {}", task.filename, err)
                        }
                    }
                    return
                }
            };
            let mut flow_control = body.flow_control().clone();
            while let Some(chunk) = body.data().await {
                let chunk = chunk.unwrap();
                println!("handler {:?} status: {} RX: {:?} for {}", id, head.status, chunk.len(), task.filename); // TODO write to the file

                // Let the server send more data.
                let _ = flow_control.release_capacity(chunk.len());
            }
        });
        handles.push(handle);
        streams_used += 1;
        if handles.len() % 20 == 0 {
            println!("sleeping {} between batches", BATCH_DELAY);
            sleep(Duration::from_millis(BATCH_DELAY)).await;
            // Update the list of handles to remove completed tasks
            handles.retain(|handle| !handle.is_finished());
        }
    }

    println!("waiting for feeder to finish");
    feeder.await.expect("error waiting for feeder to finish");

    while !handles.is_empty() {
        println!("{} tasks not finished yet", handles.len());

        sleep(Duration::from_millis(1000)).await;
        handles.retain(|handle| !handle.is_finished());
    }
    println!("all {} tasks are finished. {} connection were established", streams_used, connection_established);
}

fn join_mpsc<T: Send + Display + 'static>(
    mut receiver1: mpsc::Receiver<T>,
    mut receiver2: mpsc::Receiver<T>,
) -> mpsc::Receiver<T> {
    let (sender, receiver) = mpsc::channel::<T>(32);

    tokio::spawn(async move {
        loop {
            let msg = select! {
                Some(msg) = receiver1.recv() =>  {
                    println!("================== got message from the priority receiver: {}", msg);
                    msg
                }
                Some(msg) = receiver2.recv() => {
                    msg
                }
                else => {
                    eprintln!("both channels closed - aborting");
                    break;
                }
            };
            if let Err(err) = sender.send(msg).await {
                eprintln!("failed sending to combined channel: {}", err);
                break
            }
        }
        println!("finishing combined channel coroutine")
    });

    receiver
}