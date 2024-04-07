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
use tokio::task::JoinHandle;

#[derive(Clone)]
struct Task {
    filename: String,
    retry_sender: mpsc::Sender<Task>,
}

impl Task {
    async fn retry(self) {
        match self.retry_sender.send(self.clone()).await {
            Ok(_) => {
                eprintln!("successfully sent {} for retry", self.filename)
            }
            Err(err) => {
                eprintln!("failed to send {} for retry: {}", self.filename, err)
            }
        }
    }
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

    // it's important: retry_sender doesn't get here
    let (mut task_receiver, feeder_handle) = task_producer(rx, 256, 1024);

    let mut h2connection: Option<Connection> = None;
    let mut receiver: Option<mpsc::Receiver<bool>> = None;

    let mut handles = Vec::new();
    let mut streams_used: usize = 0;
    let mut connection_established: usize = 0;


    while let Some(task) = task_receiver.recv().await {

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
                    task.retry().await;
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

    // TODO: consider removing this await call
    println!("waiting for feeder to finish");
    feeder_handle.await.expect("error waiting for feeder to finish");

    while !handles.is_empty() {
        println!("{} tasks not finished yet", handles.len());

        sleep(Duration::from_millis(1000)).await;
        handles.retain(|handle| !handle.is_finished());
    }
    println!("all {} tasks are finished. {} connection were established", streams_used, connection_established);
}

fn task_producer(rx: Receiver<String>, source_buffer: usize, retry_buffer: usize) -> (mpsc::Receiver<Task>, JoinHandle<()>) {
    let (input_sender, mut input_receiver) = mpsc::channel::<Task>(source_buffer);
    let (retry_sender, mut retry_receiver) = mpsc::channel::<Task>(retry_buffer);

    let (sender, combined_receiver) = mpsc::channel(32);

    let feeder_handler = tokio::spawn(async move {
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

    let combined_handle= tokio::spawn(async move {
        loop {
            let msg = select! {
                biased;
                Some(msg) = retry_receiver.recv() =>  {
                    msg
                }
                Some(msg) = input_receiver.recv() => {
                    msg
                }
                else => {
                    break;
                }
            };
            if let Err(err) = sender.send(msg).await {
                eprintln!("failed sending to combined channel: {}", err);
                break
            }
        }
    });

    (
        combined_receiver,
        tokio::spawn(async move {
            match combined_handle.await {
                Ok(_) => {}
                Err(_) => {
                    return
                }
            }
            match feeder_handler.await {
                Ok(_) => {}
                Err(_) => {}
            }
            return
        }),
    )
}